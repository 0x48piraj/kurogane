//! Packages the canonical Kurogane bundle as an NSIS installer.

use anyhow::{Result, bail};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use kurogane_layout::{
    PackagingConfig, ResolvedDistribution, SignConfig, package_directory, sign_artifact, sign_tree,
    verify_signature,
};

use crate::tui;

/// NSIS template that installs the canonical directory bundle as an opaque payload.
const INSTALLER_NSI: &str = r#"Unicode true
ManifestDPIAware true

SetCompressor /SOLID lzma

!include MUI2.nsh
!include FileFunc.nsh
!include x64.nsh

!define PRODUCTNAME "{{product_name}}"
!define VERSION "{{version}}"
!define MAINBINARYNAME "{{main_binary_name}}"
!define COPYRIGHT "{{copyright}}"
!define FILEDESCRIPTION "{{file_description}}"
!define OUTFILE "{{out_file}}"
!define ARCH "{{arch}}"
!define BUNDLEDIR "{{bundle_dir}}"
!define MANUFACTURER "{{manufacturer}}"
!define ESTIMATEDSIZE "{{estimated_size}}"

Name "${PRODUCTNAME}"
BrandingText "${COPYRIGHT}"
OutFile "${OUTFILE}"

InstallDir "$LOCALAPPDATA\Programs\${PRODUCTNAME}"

VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "${PRODUCTNAME}"
VIAddVersionKey "FileDescription" "${FILEDESCRIPTION}"
VIAddVersionKey "LegalCopyright" "${COPYRIGHT}"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "ProductVersion" "${VERSION}"

Var PassiveMode

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

Function .onInit
    ${GetParameters} $0
    ${GetOptions} $0 "/S" $PassiveMode
FunctionEnd

Section "Install"
    SetOutPath $INSTDIR

    ; Canonical bundle, installed wholesale
    File /r "${BUNDLEDIR}\*.*"

{{start_menu_shortcut}}{{desktop_shortcut}}
    ; Uninstaller
    WriteUninstaller "$INSTDIR\uninstall.exe"

    ; Add/Remove Programs registry
    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCTNAME}" "DisplayName" "${PRODUCTNAME}"
    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCTNAME}" "UninstallString" '"$INSTDIR\uninstall.exe"'
    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCTNAME}" "InstallLocation" "$INSTDIR"
    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCTNAME}" "DisplayVersion" "${VERSION}"
    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCTNAME}" "Publisher" "${MANUFACTURER}"
    WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCTNAME}" "NoModify" 1
    WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCTNAME}" "NoRepair" 1
    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCTNAME}" "EstimatedSize" "${ESTIMATEDSIZE}"
SectionEnd

Section "Uninstall"
    ; Remove shortcuts
    Delete "$SMPROGRAMS\${PRODUCTNAME}\${PRODUCTNAME}.lnk"
    Delete "$SMPROGRAMS\${PRODUCTNAME}\Uninstall ${PRODUCTNAME}.lnk"
    RMDir "$SMPROGRAMS\${PRODUCTNAME}"
    Delete "$DESKTOP\${PRODUCTNAME}.lnk"

    ; Remove everything that was installed
    RMDir /r "$INSTDIR"

    ; Remove registry keys
    DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCTNAME}"
SectionEnd
"#;

fn find_makensis() -> Result<PathBuf> {
    // Check NSIS_PATH env var
    if let Ok(path) = std::env::var("NSIS_PATH") {
        let p = PathBuf::from(path);
        let makensis = if p.is_dir() {
            p.join("makensis.exe")
        } else {
            p
        };
        if makensis.exists() {
            return Ok(makensis);
        }
    }

    // Check common Windows install locations
    #[cfg(target_os = "windows")]
    {
        let program_files = std::env::var("ProgramFiles")
            .or_else(|_| std::env::var("PROGRAMFILES"))
            .unwrap_or_default();
        let program_files_x86 = std::env::var("ProgramFiles(x86)").unwrap_or_default();

        for base in [&program_files, &program_files_x86] {
            let makensis = PathBuf::from(base).join("NSIS").join("makensis.exe");
            if makensis.exists() {
                return Ok(makensis);
            }
        }
    }

    // Check system makensis via which
    if let Ok(output) = Command::new("which").arg("makensis").output()
        && output.status.success()
    {
        let makensis_str = String::from_utf8_lossy(&output.stdout);
        let makensis_path = PathBuf::from(makensis_str.trim());
        if makensis_path.exists() {
            return Ok(makensis_path);
        }
    }

    bail!("NSIS not found. Install NSIS or set NSIS_PATH environment variable.");
}

/// Start Menu shortcut block, emitted when `windows.start-menu-shortcut` is on.
const START_MENU_SHORTCUT_BLOCK: &str = r#"    ; Start Menu shortcut
    CreateDirectory "$SMPROGRAMS\${PRODUCTNAME}"
    CreateShortCut "$SMPROGRAMS\${PRODUCTNAME}\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}"
    CreateShortCut "$SMPROGRAMS\${PRODUCTNAME}\Uninstall ${PRODUCTNAME}.lnk" "$INSTDIR\uninstall.exe"

"#;

/// Desktop shortcut block, emitted when `windows.desktop-shortcut` is on.
const DESKTOP_SHORTCUT_BLOCK: &str = r#"    ; Desktop shortcut
    CreateShortCut "$DESKTOP\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}"

"#;

/// Generates the NSIS script for a staged canonical bundle.
fn generate_installer_nsi(
    dist: &ResolvedDistribution,
    config: &PackagingConfig,
    bundle_dir: &Path,
    output_dir: &Path,
) -> Result<PathBuf> {
    let name = &dist.metadata.name;
    let version = &dist.metadata.version;
    let exe_name = &dist.metadata.exe_name;

    let bundle_source = bundle_dir
        .strip_prefix(output_dir)
        .map_err(|_| {
            anyhow::anyhow!(
                "bundle directory {} is not inside NSIS output directory {}",
                bundle_dir.display(),
                output_dir.display()
            )
        })?
        .to_string_lossy()
        .replace('/', "\\");
    let arch = installer_arch();
    let out_file = format!("{name}_{version}_{arch}-setup.exe");

    let estimated_size = dir_size(bundle_dir)? / 1024;

    let copyright = dist
        .metadata
        .copyright
        .clone()
        .unwrap_or_else(|| format!("{name} {version}"));
    let manufacturer = dist
        .metadata
        .publisher
        .clone()
        .unwrap_or_else(|| name.clone());
    let file_description = dist
        .metadata
        .description
        .clone()
        .unwrap_or_else(|| name.clone());
    let start_menu_shortcut = if config.windows.start_menu_shortcut {
        START_MENU_SHORTCUT_BLOCK
    } else {
        ""
    };
    let desktop_shortcut = if config.windows.desktop_shortcut {
        DESKTOP_SHORTCUT_BLOCK
    } else {
        ""
    };

    // These values are interpolated and must be escaped
    let nsi_content = INSTALLER_NSI
        .replace("{{product_name}}", &escape_nsis(name))
        .replace("{{version}}", &escape_nsis(version))
        .replace("{{main_binary_name}}", &escape_nsis(exe_name))
        .replace("{{copyright}}", &escape_nsis(&copyright))
        .replace("{{file_description}}", &escape_nsis(&file_description))
        .replace("{{out_file}}", &escape_nsis(&out_file))
        .replace("{{arch}}", &escape_nsis(arch))
        .replace("{{bundle_dir}}", &escape_nsis(&bundle_source))
        .replace("{{manufacturer}}", &escape_nsis(&manufacturer))
        .replace("{{estimated_size}}", &estimated_size.to_string())
        .replace("{{start_menu_shortcut}}", start_menu_shortcut)
        .replace("{{desktop_shortcut}}", desktop_shortcut);

    let nsi_path = output_dir.join("installer.nsi");
    fs::write(&nsi_path, &nsi_content)?;
    Ok(nsi_path)
}

/// Escapes a value for use in a quoted NSIS string.
///
/// Escapes `$` and `"` as `$$` and `$"`, respectively.
/// Backslashes are preserved.
///
/// Newlines are replaced with spaces; a `!define` cannot span lines.
fn escape_nsis(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());

    for ch in value.chars() {
        match ch {
            // Quote escaping introduces `$`, so handle `$` first
            '$' => escaped.push_str("$$"),
            '"' => escaped.push_str("$\\\""),
            '\r' | '\n' => escaped.push(' '),
            _ => escaped.push(ch),
        }
    }

    escaped
}

/// Resolves the installer architecture, preferring `ARCH` when provided.
fn installer_arch() -> &'static str {
    classify_arch(std::env::var("ARCH").ok().as_deref())
}

/// Classifies the installer architecture from an optional `ARCH` value.
///
/// Unset or empty values fall back to the compilation target.
fn classify_arch(env_arch: Option<&str>) -> &'static str {
    match env_arch.map(str::to_ascii_lowercase).as_deref() {
        Some(arch) if !arch.is_empty() => {
            if arch.contains("aarch64") || arch.contains("arm64") {
                "arm64"
            } else if arch.contains("64") || arch.contains("amd64") {
                "x64"
            } else {
                "x86"
            }
        }
        _ => target_arch(),
    }
}

fn target_arch() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "x64"
    } else if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x86"
    }
}

fn dir_size(path: &Path) -> Result<u64> {
    let mut total = 0;
    if path.is_dir() {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            if metadata.is_dir() {
                total += dir_size(&entry.path())?;
            } else {
                total += metadata.len();
            }
        }
    }
    Ok(total)
}

/// Builds a Windows NSIS installer from the canonical directory bundle.
pub fn build(
    dist: &ResolvedDistribution,
    output_dir: &Path,
    config: &PackagingConfig,
    sign: Option<&SignConfig>,
) -> Result<()> {
    let makensis = find_makensis()?;

    if output_dir.exists() {
        fs::remove_dir_all(output_dir)?;
    }
    fs::create_dir_all(output_dir)?;

    let name = &dist.metadata.name;
    let version = &dist.metadata.version;
    let arch = installer_arch();
    let installer_name = format!("{name}_{version}_{arch}-setup.exe");

    tui::step("Staging bundle...");

    // Stage the bundle for installer assembly
    let bundle_dir = output_dir.join("bundle");
    package_directory(dist, &bundle_dir)?;

    // Sign staged binaries using configured signing policy
    if let Some(sign_config) = sign {
        let signed = sign_tree(&bundle_dir, sign_config)?;
        tui::field("signed", format!("{signed} file(s)"));
    }

    tui::step("Generating installer script...");

    // Generate .nsi
    let nsi_path = generate_installer_nsi(dist, config, &bundle_dir, output_dir)?;

    tui::step("Compiling installer...");

    // Compile
    let status = Command::new(&makensis)
        .args(["-INPUTCHARSET", "UTF8", "-OUTPUTCHARSET", "UTF8"])
        .arg("-V2")
        .arg(nsi_path.file_name().unwrap())
        .current_dir(output_dir)
        .status()?;

    if !status.success() {
        let code = status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "signal".into());
        bail!("makensis failed (exit code: {code})");
    }

    let installer_path = output_dir.join(&installer_name);
    if installer_path.exists() {
        tui::field("installer", tui::format_path(&installer_path));
    }

    // Cleanup staging
    fs::remove_dir_all(&bundle_dir)?;

    // Sign and verify the resulting artifact
    if let Some(sign_config) = sign {
        if !installer_path.exists() {
            bail!("installer {} was not produced", installer_path.display());
        }

        sign_artifact(&installer_path, sign_config)?;
        verify_signature(&installer_path, sign_config)?;
        tui::field("signature", "verified");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escaping_leaves_ordinary_values_untouched() {
        assert_eq!(escape_nsis("My App"), "My App");
        assert_eq!(escape_nsis("1.2.3"), "1.2.3");
    }

    #[test]
    fn escaping_preserves_windows_path_separators() {
        assert_eq!(escape_nsis(r"bundle\runtime"), r"bundle\runtime");
    }

    #[test]
    fn quotes_cannot_terminate_the_define_early() {
        assert_eq!(escape_nsis(r#"Acme "Corp""#), r#"Acme $\"Corp$\""#);
    }

    #[test]
    fn dollars_cannot_introduce_an_nsis_variable() {
        assert_eq!(escape_nsis("$INSTDIR"), "$$INSTDIR");
        assert_eq!(escape_nsis("100$"), "100$$");
    }

    #[test]
    fn newlines_cannot_split_a_define_across_lines() {
        let escaped = escape_nsis("Acme\nCorp\r\nLtd");

        assert!(!escaped.contains('\n'));
        assert!(!escaped.contains('\r'));
        assert_eq!(escaped, "Acme Corp  Ltd");
    }

    #[test]
    fn generated_script_escapes_a_hostile_publisher() {
        let dir = tmp();
        let mut dist = test_distribution(dir.path());
        dist.metadata.publisher = Some(r#"Evil" ; MessageBox MB_OK "pwned"#.to_string());

        let out = dir.path().join("out");
        let bundle = out.join("bundle");
        std::fs::create_dir_all(&bundle).unwrap();

        let nsi =
            generate_installer_nsi(&dist, &PackagingConfig::default(), &bundle, &out).unwrap();
        let content = std::fs::read_to_string(nsi).unwrap();

        let manufacturer = content
            .lines()
            .find(|line| line.starts_with("!define MANUFACTURER "))
            .expect("MANUFACTURER define should be present");

        assert!(
            manufacturer.ends_with('"') && manufacturer.matches("$\\\"").count() == 2,
            "embedded quotes must be escaped, not closing the define: {manufacturer}"
        );
    }

    fn tmp() -> tempfile::TempDir {
        kurogane_layout::test_fixtures::tmp_dir()
    }

    fn test_distribution(dir: &Path) -> ResolvedDistribution {
        kurogane_layout::test_fixtures::sample_distribution(dir)
    }

    fn generated_nsi(dir: &Path) -> String {
        let dist = test_distribution(dir);
        let bundle = dir.join("bundle");
        fs::create_dir_all(&bundle).unwrap();

        let nsi = generate_installer_nsi(&dist, &PackagingConfig::default(), &bundle, dir).unwrap();
        fs::read_to_string(nsi).unwrap()
    }

    /// Generates an NSIS script from the supplied packaging configuration.
    fn generated_nsi_with(dir: &Path, config: &PackagingConfig) -> String {
        let mut dist = test_distribution(dir);
        config.app.apply_to(&mut dist.metadata);
        let bundle = dir.join("bundle");
        fs::create_dir_all(&bundle).unwrap();

        let nsi = generate_installer_nsi(&dist, config, &bundle, dir).unwrap();
        fs::read_to_string(nsi).unwrap()
    }

    #[test]
    fn dir_size_counts_files() {
        let dir = tmp();
        fs::write(dir.path().join("a.txt"), "hello").unwrap();
        fs::write(dir.path().join("b.txt"), "world!").unwrap();

        let size = dir_size(dir.path()).unwrap();
        assert_eq!(size, 11); // 5 + 6
    }

    #[test]
    fn dir_size_counts_nested() {
        let dir = tmp();
        let sub = dir.path().join("sub");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("a.txt"), "hello").unwrap();
        fs::write(dir.path().join("b.txt"), "world").unwrap();

        let size = dir_size(dir.path()).unwrap();
        assert_eq!(size, 10); // 5 + 5
    }

    #[test]
    fn dir_size_empty_dir() {
        let dir = tmp();
        let size = dir_size(dir.path()).unwrap();
        assert_eq!(size, 0);
    }

    #[test]
    fn arm64_env_values_are_not_classified_as_x64() {
        assert_eq!(classify_arch(Some("aarch64")), "arm64");
        assert_eq!(classify_arch(Some("arm64")), "arm64");
        assert_eq!(classify_arch(Some("ARM64")), "arm64");
    }

    #[test]
    fn x86_64_env_values_are_classified_as_x64() {
        assert_eq!(classify_arch(Some("x86_64")), "x64");
        assert_eq!(classify_arch(Some("amd64")), "x64");
        assert_eq!(classify_arch(Some("AMD64")), "x64");
        assert_eq!(classify_arch(Some("x64")), "x64");
    }

    #[test]
    fn unknown_32_bit_env_value_is_classified_as_x86() {
        assert_eq!(classify_arch(Some("x86")), "x86");
        assert_eq!(classify_arch(Some("i686")), "x86");
    }

    #[test]
    fn unset_or_empty_arch_falls_back_to_target() {
        assert_eq!(classify_arch(None), target_arch());
        assert_eq!(classify_arch(Some("")), target_arch());
    }

    #[test]
    fn generate_nsi_installs_bundle_wholesale() {
        let dir = tmp();
        let content = generated_nsi(dir.path());

        assert!(
            content.contains(r#"File /r "${BUNDLEDIR}\*.*""#),
            "installer must copy the canonical bundle as-is"
        );
    }

    #[test]
    fn generate_nsi_has_no_legacy_component_defines() {
        let dir = tmp();
        let content = generated_nsi(dir.path());

        for legacy in [
            "CEFDIR",
            "CONTENTDIR",
            "RESOURCESDIR",
            "HASCONTENT",
            "HASRESOURCES",
        ] {
            assert!(
                !content.contains(legacy),
                "template must not carry legacy define {legacy}"
            );
        }
    }

    #[test]
    fn generate_nsi_uninstall_removes_whole_install_dir() {
        let dir = tmp();
        let content = generated_nsi(dir.path());

        assert!(content.contains(r#"RMDir /r "$INSTDIR""#));
    }

    #[test]
    fn generate_nsi_outfile_contains_name_version_arch() {
        let dir = tmp();
        let content = generated_nsi(dir.path());

        let arch = installer_arch();
        assert!(content.contains(&format!("myapp_1.0.0_{arch}-setup.exe")));
    }

    #[test]
    fn generate_nsi_defines_main_executable() {
        let dir = tmp();
        let dist = test_distribution(dir.path());
        let content = generated_nsi(dir.path());

        assert!(content.contains(&format!(
            r#"!define MAINBINARYNAME "{}""#,
            dist.metadata.exe_name
        )));
    }

    #[test]
    fn generate_nsi_defaults_match_historical_metadata() {
        let dir = tmp();
        let content = generated_nsi(dir.path());

        assert!(
            content.contains(r#"!define COPYRIGHT "myapp 1.0.0""#),
            "copyright must default to '<name> <version>'"
        );
        assert!(
            content.contains(r#"!define MANUFACTURER "myapp""#),
            "manufacturer must default to the product name"
        );
        assert!(
            content.contains(r#"!define FILEDESCRIPTION "myapp""#),
            "file description must default to the product name"
        );
    }

    #[test]
    fn generate_nsi_defaults_include_both_shortcuts() {
        let dir = tmp();
        let content = generated_nsi(dir.path());

        assert!(content.contains("; Start Menu shortcut"));
        assert!(content.contains(r#"CreateShortCut "$SMPROGRAMS\${PRODUCTNAME}\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}""#));
        assert!(content.contains(
            r#"CreateShortCut "$DESKTOP\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}""#
        ));
    }

    #[test]
    fn generate_nsi_identity_overrides_flow_through_metadata() {
        let dir = tmp();

        let config = PackagingConfig {
            app: kurogane_layout::AppConfig {
                publisher: Some("Example Corp".into()),
                description: Some("A demo application".into()),
                copyright: Some("(c) 2026 Example Corp".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let content = generated_nsi_with(dir.path(), &config);

        assert!(content.contains(r#"!define MANUFACTURER "Example Corp""#));
        assert!(content.contains(r#"Publisher" "${MANUFACTURER}"#));
        assert!(content.contains(r#"!define FILEDESCRIPTION "A demo application""#));
        assert!(content.contains(r#"!define COPYRIGHT "(c) 2026 Example Corp""#));
        assert!(
            !content.contains("myapp 1.0.0"),
            "default copyright fallback must not appear when overridden"
        );
    }

    #[test]
    fn generate_nsi_shortcut_toggles_remove_blocks() {
        let dir = tmp();

        let config = PackagingConfig {
            windows: kurogane_layout::WindowsPackagingConfig {
                start_menu_shortcut: false,
                desktop_shortcut: false,
            },
            ..Default::default()
        };
        let content = generated_nsi_with(dir.path(), &config);

        assert!(
            !content.contains("CreateShortCut"),
            "no shortcut creation may remain when both toggles are off"
        );
        assert!(
            !content.contains("; Start Menu shortcut") && !content.contains("; Desktop shortcut"),
            "shortcut comment banners must be removed with their blocks"
        );

        // Uninstall section still cleans up any pre-existing shortcuts harmlessly
        assert!(content.contains(r#"Delete "$DESKTOP\${PRODUCTNAME}.lnk""#));
    }

    #[test]
    fn generate_nsi_start_menu_only_shortcut() {
        let dir = tmp();

        let config = PackagingConfig {
            windows: kurogane_layout::WindowsPackagingConfig {
                start_menu_shortcut: true,
                desktop_shortcut: false,
            },
            ..Default::default()
        };
        let content = generated_nsi_with(dir.path(), &config);

        assert!(content.contains("$SMPROGRAMS"));
        assert!(!content.contains(r#"CreateShortCut "$DESKTOP"#));
    }
}

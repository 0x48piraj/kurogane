use anyhow::{Result, bail};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use kurogane_layout::{ResolvedDistribution, package_directory};

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
!define OUTFILE "{{out_file}}"
!define ARCH "{{arch}}"
!define BUNDLEDIR "{{bundle_dir}}"
!define INSTALLMODE "{{install_mode}}"
!define MANUFACTURER "{{manufacturer}}"
!define ESTIMATEDSIZE "{{estimated_size}}"

Name "${PRODUCTNAME}"
BrandingText "${COPYRIGHT}"
OutFile "${OUTFILE}"

!define PLACEHOLDER_INSTALL_DIR "placeholder\${PRODUCTNAME}"
InstallDir "${PLACEHOLDER_INSTALL_DIR}"

VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "${PRODUCTNAME}"
VIAddVersionKey "FileDescription" "${PRODUCTNAME}"
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

    ; Start Menu shortcut
    CreateDirectory "$SMPROGRAMS\${PRODUCTNAME}"
    CreateShortCut "$SMPROGRAMS\${PRODUCTNAME}\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}"
    CreateShortCut "$SMPROGRAMS\${PRODUCTNAME}\Uninstall ${PRODUCTNAME}.lnk" "$INSTDIR\uninstall.exe"

    ; Desktop shortcut
    CreateShortCut "$DESKTOP\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}"

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

/// Generates the NSIS script for a staged canonical bundle.
fn generate_installer_nsi(
    dist: &ResolvedDistribution,
    bundle_dir: &Path,
    output_dir: &Path,
) -> Result<PathBuf> {
    let name = &dist.metadata.name;
    let version = &dist.metadata.version;
    let exe_name = &dist.metadata.exe_name;

    let bundle_source = bundle_dir
        .strip_prefix(output_dir)
        .map_err(|_| anyhow::anyhow!(
            "bundle directory {} is not inside NSIS output directory {}",
            bundle_dir.display(),
            output_dir.display()
        ))?
        .to_string_lossy()
        .replace('/', "\\");
    let arch = installer_arch();
    let out_file = format!("{name}_{version}_{arch}-setup.exe");

    let estimated_size = dir_size(bundle_dir)? / 1024;

    let nsi_content = INSTALLER_NSI
        .replace("{{product_name}}", name)
        .replace("{{version}}", version)
        .replace("{{main_binary_name}}", exe_name)
        .replace("{{copyright}}", &format!("{} {}", name, version))
        .replace("{{out_file}}", &out_file)
        .replace("{{arch}}", arch)
        .replace("{{bundle_dir}}", &bundle_source)
        .replace("{{install_mode}}", "currentUser")
        .replace("{{manufacturer}}", name)
        .replace("{{estimated_size}}", &estimated_size.to_string());

    let nsi_path = output_dir.join("installer.nsi");
    fs::write(&nsi_path, &nsi_content)?;
    Ok(nsi_path)
}

/// Resolves the installer architecture, preferring `ARCH` when provided.
fn installer_arch() -> &'static str {
    match std::env::var("ARCH") {
        Ok(arch) if !arch.is_empty() => {
            if arch.contains("64") || arch.contains("x86_64") || arch.contains("amd64") {
                "x64"
            } else {
                "x86"
            }
        }
        _ => {
            if cfg!(target_arch = "x86_64") {
                "x64"
            } else if cfg!(target_arch = "aarch64") {
                "arm64"
            } else {
                "x86"
            }
        }
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
///
/// The bundle is staged unchanged, wrapped in an NSIS installer and then
/// compiled with `makensis`.
pub fn build(dist: &ResolvedDistribution, output_dir: &Path) -> Result<()> {
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

    // Materialize the canonical bundle
    let bundle_dir = output_dir.join("bundle");
    package_directory(dist, &bundle_dir)?;

    tui::step("Generating installer script...");

    // Generate .nsi
    let nsi_path = generate_installer_nsi(dist, &bundle_dir, output_dir)?;

    tui::step("Compiling installer...");

    // Compile
    let status = Command::new(&makensis)
        .args(["-INPUTCHARSET", "UTF8", "-OUTPUTCHARSET", "UTF8"])
        .arg("-V2")
        .arg(nsi_path.file_name().unwrap())
        .current_dir(output_dir)
        .status()?;

    if !status.success() {
        bail!("makensis failed");
    }

    let installer_path = output_dir.join(&installer_name);
    if installer_path.exists() {
        tui::field("installer", tui::format_path(&installer_path));
    }

    // Cleanup staging
    fs::remove_dir_all(&bundle_dir)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurogane_layout::AppMetadata;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().expect("failed to create temp dir")
    }

    fn create_cef_fixture(dir: &Path) -> PathBuf {
        let cef = dir.join("cef");
        fs::create_dir_all(&cef).unwrap();
        fs::write(cef.join("libcef.dll"), "cef").unwrap();
        fs::write(cef.join("chrome_elf.dll"), "elf").unwrap();
        fs::write(cef.join("icudtl.dat"), "icu").unwrap();
        fs::write(cef.join("v8_context_snapshot.bin"), "v8").unwrap();
        fs::create_dir_all(cef.join("locales")).unwrap();
        fs::write(cef.join("locales").join("en-US.pak"), "pak").unwrap();
        cef
    }

    fn test_distribution(dir: &Path) -> ResolvedDistribution {
        #[cfg(target_os = "windows")]
        let exe_name = "myapp.exe";
        #[cfg(not(target_os = "windows"))]
        let exe_name = "myapp";

        let exe = dir.join(exe_name);
        fs::write(&exe, "binary").unwrap();

        let frontend = dir.join("content");
        fs::create_dir_all(&frontend).unwrap();
        fs::write(frontend.join("index.html"), "<html></html>").unwrap();

        let cef = create_cef_fixture(dir);

        ResolvedDistribution {
            metadata: AppMetadata {
                name: "myapp".to_string(),
                version: "1.0.0".to_string(),
                exe_name: exe_name.to_string(),
            },
            executable: exe,
            frontend: Some(frontend),
            cef_runtime: cef,
            extra_resources: Vec::new(),
        }
    }

    fn generated_nsi(dir: &Path) -> String {
        let dist = test_distribution(dir);
        let bundle = dir.join("bundle");
        fs::create_dir_all(&bundle).unwrap();

        let nsi = generate_installer_nsi(&dist, &bundle, dir).unwrap();
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

        for legacy in ["CEFDIR", "CONTENTDIR", "RESOURCESDIR", "HASCONTENT", "HASRESOURCES"] {
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
}

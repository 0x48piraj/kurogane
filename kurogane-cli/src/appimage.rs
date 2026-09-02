//! Linux AppImage generation.
//!
//! This module constructs an AppDir around the canonical Kurogane
//! bundle and uses linuxdeploy to produce the AppImage artifact.

use anyhow::{Result, bail};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use kurogane_layout::{PackagingConfig, ResolvedDistribution, SignConfig, package_directory, sign_tree};

use crate::tui;

const DOWNLOAD_LIMIT: u64 = 32 * 1024 * 1024;

const LINUXDEPLOY_VERSION: &str = "1-alpha-20251107-1";

const LINUXDEPLOY_URL: &str = "https://github.com/linuxdeploy/linuxdeploy/releases/download";

/// SHA-256 digests of the pinned linuxdeploy release assets.
///
/// These must be updated together with [`LINUXDEPLOY_VERSION`].
const LINUXDEPLOY_DIGESTS: &[(&str, &str)] = &[
    (
        "x86_64",
        "c20cd71e3a4e3b80c3483cef793cda3f4e990aca14014d23c544ca3ce1270b4d",
    ),
    (
        "aarch64",
        "620095110d693282b8ebeb244a95b5e911cf8f65f76c88b4b47d16ae6346fcff",
    ),
];

fn expected_digest(arch: &str) -> Result<&'static str> {
    LINUXDEPLOY_DIGESTS
        .iter()
        .find(|(candidate, _)| *candidate == arch)
        .map(|(_, digest)| *digest)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no pinned linuxdeploy digest for architecture '{arch}' \
                 (linuxdeploy {LINUXDEPLOY_VERSION})"
            )
        })
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};

    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn tools_arch() -> Result<String> {
    match std::env::var("ARCH") {
        Ok(arch) if !arch.is_empty() => Ok(arch),
        _ => {
            let output = Command::new("uname").arg("-m").output()?;
            let arch = String::from_utf8_lossy(&output.stdout).trim().to_string();
            match arch.as_str() {
                "x86_64" | "amd64" => Ok("x86_64".to_string()),
                "aarch64" | "arm64" => Ok("aarch64".to_string()),
                _ => bail!("unsupported architecture: {arch}"),
            }
        }
    }
}

fn tools_dir() -> Result<PathBuf> {
    let dir = dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("kurogane")
        .join("tools");
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn write_and_make_executable(path: &Path, data: &[u8]) -> Result<()> {
    fs::write(path, data)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    Ok(())
}

fn download(url: &str) -> Result<Vec<u8>> {
    let mut response = ureq::get(url).call()?;

    Ok(response
        .body_mut()
        .with_config()
        .limit(DOWNLOAD_LIMIT)
        .read_to_vec()?)
}

/// Downloads and caches the pinned linuxdeploy release.
///
/// Downloads are verified against [`LINUXDEPLOY_DIGESTS`] before being written
/// to the cache. Cached copies are not re-hashed because [`patch_linuxdeploy`]
/// three bytes of the file in place.
fn prepare_linuxdeploy(arch: &str) -> Result<PathBuf> {
    let tools = tools_dir()?;

    // linuxdeploy
    let path = tools.join(format!("linuxdeploy-{arch}.AppImage"));
    if !path.exists() {
        let expected = expected_digest(arch)?;

        tui::step(&format!("Downloading linuxdeploy-{arch}..."));
        let data = download(&format!(
            "{LINUXDEPLOY_URL}/{LINUXDEPLOY_VERSION}/linuxdeploy-{arch}.AppImage"
        ))?;

        let actual = sha256_hex(&data);
        if actual != expected {
            bail!(
                "linuxdeploy-{arch} failed checksum verification\n  \
                 expected: {expected}\n  \
                 actual:   {actual}\n\
                 Refusing to execute an unverified build tool."
            );
        }
        tui::field("sha256", &actual[..16]);

        write_and_make_executable(&path, &data)?;
        // Mask linuxdeploy's magic bytes
        patch_linuxdeploy(&path)?;
    }

    Ok(path)
}

/// Disables AppImage execution metadata in the linuxdeploy binary.
///
/// linuxdeploy is itself distributed as an AppImage. Clearing the three
/// bytes at offset 8 allows it to be executed in environments where the
/// AppImage ELF metadata interferes with the host loader.
fn patch_linuxdeploy(path: &Path) -> Result<()> {
    let status = Command::new("dd")
        .args([
            "if=/dev/zero",
            "bs=1",
            "count=3",
            "seek=8",
            "conv=notrunc",
            &format!("of={}", path.display()),
        ])
        .status()?;

    if !status.success() {
        bail!("failed to prepare linuxdeploy");
    }

    Ok(())
}

/// Generates the AppRun entrypoint for the canonical Kurogane bundle.
fn generate_apprun(name: &str, exe_name: &str) -> String {
    format!(
        r#"#!/bin/sh
APPDIR="$(dirname "$(readlink -f "$0")")"
exec "$APPDIR/usr/lib/{name}/{exe_name}" "$@"
"#
    )
}

/// Generates the desktop entry consumed by AppImage tooling.
fn generate_desktop(
    name: &str,
    exe_name: &str,
    version: &str,
    categories: &[String],
    terminal: bool,
) -> String {
    let categories = if categories.is_empty() {
        "Utility".to_string()
    } else {
        categories.join(";")
    };

    // `Version` is the spec version; the app version uses `X-AppImage-Version`
    format!(
        r#"[Desktop Entry]
Type=Application
Name={name}
Version=1.0
X-AppImage-Version={version}
Exec={exe_name}
Icon={name}
Categories={categories};
Terminal={terminal}
"#
    )
}

/// Builds the AppDir around the canonical Kurogane directory bundle.
fn build_appdir(
    dist: &ResolvedDistribution,
    app_dir: &Path,
    config: &PackagingConfig,
) -> Result<()> {
    let name = &dist.metadata.name;
    let exe_name = &dist.metadata.exe_name;

    // Canonical Kurogane bundle
    let bundle_root = app_dir.join("usr").join("lib").join(name);
    package_directory(dist, &bundle_root)?;

    // AppImage entrypoint
    let apprun_content = generate_apprun(name, exe_name);
    let apprun_path = app_dir.join("AppRun");
    fs::write(&apprun_path, &apprun_content)?;
    fs::set_permissions(&apprun_path, fs::Permissions::from_mode(0o755))?;

    // Desktop entry
    let categories = config.linux.categories.as_deref().unwrap_or_default();
    let terminal = config.linux.terminal.unwrap_or(false);
    let desktop_content =
        generate_desktop(name, exe_name, &dist.metadata.version, categories, terminal);
    let desktop_dir = app_dir.join("usr").join("share").join("applications");
    fs::create_dir_all(&desktop_dir)?;
    fs::write(
        desktop_dir.join(format!("{name}.desktop")),
        &desktop_content,
    )?;

    // Icon in the hicolor theme
    let icon_dir = app_dir
        .join("usr")
        .join("share")
        .join("icons")
        .join("hicolor")
        .join("256x256")
        .join("apps");
    fs::create_dir_all(&icon_dir)?;
    let icon_path = icon_dir.join(format!("{name}.png"));

    match &dist.metadata.icon {
        Some(icon) => {
            if !icon.exists() {
                bail!("configured icon not found: {}", icon.display());
            }
            fs::copy(icon, &icon_path)?;
        }
        None => write_placeholder_icon(&icon_path)?,
    }

    Ok(())
}

/// Writes the placeholder icon used until application branding is configurable.
fn write_placeholder_icon(path: &Path) -> Result<()> {
    // Minimal valid 1x1 PNG
    let png: [u8; 69] = [
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90,
        0x77, 0x53, 0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x08, 0xd7, 0x63, 0xf8,
        0xcf, 0xc0, 0x00, 0x00, 0x00, 0x02, 0x00, 0x01, 0xe2, 0x21, 0xbc, 0x33, 0x00, 0x00, 0x00,
        0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];
    fs::write(path, png)?;
    Ok(())
}

/// Builds a Linux AppImage from the canonical Kurogane directory bundle.
///
/// The AppDir contains the verified bundle under `usr/lib/<name>` plus the
/// AppImage entrypoint, desktop entry and icon. linuxdeploy assembles the
/// resulting image and deploys external system dependencies; the bundled
/// CEF runtime remains owned by the Kurogane bundle.
pub fn build(
    dist: &ResolvedDistribution,
    output_dir: &Path,
    config: &PackagingConfig,
    sign: Option<&SignConfig>,
) -> Result<()> {
    let arch = tools_arch()?;

    // Clean output
    if output_dir.exists() {
        fs::remove_dir_all(output_dir)?;
    }
    fs::create_dir_all(output_dir)?;

    let appimage_name = format!("{}_{}_{arch}", dist.metadata.name, dist.metadata.version);
    let app_dir = output_dir.join(format!("{appimage_name}.AppDir"));
    let bundle_dir = app_dir.join("usr").join("lib").join(&dist.metadata.name);

    tui::step("Assembling AppDir...");
    build_appdir(dist, &app_dir, config)?;

    // Sign staged binaries before imaging
    if let Some(sign_config) = sign {
        let signed = sign_tree(&bundle_dir, sign_config)?;
        tui::field("signed", format!("{signed} file(s)"));
    }

    let appimage_path = output_dir.join(format!("{appimage_name}.AppImage"));

    tui::step("Running linuxdeploy...");

    let linuxdeploy = prepare_linuxdeploy(&arch)?;

    // Deploy external dependencies without relocating the canonical bundle
    // CEF remains in runtime/cef/, resolved through its $ORIGIN/cef RPATH
    let mut cmd = Command::new(&linuxdeploy);
    cmd.env("OUTPUT", &appimage_path);
    cmd.env("ARCH", &arch);
    cmd.env("APPIMAGE_EXTRACT_AND_RUN", "1");
    cmd.arg("--appimage-extract-and-run");
    cmd.arg("--appdir").arg(&app_dir);
    cmd.arg("--deploy-deps-only").arg(&bundle_dir);
    cmd.arg("--exclude-library").arg("libcef*");
    cmd.args(["--output", "appimage"]);

    let status = cmd.status()?;
    if !status.success() {
        let code = status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "signal".into());
        bail!("linuxdeploy failed (exit code: {code})");
    }

    // Remove intermediate AppDir
    fs::remove_dir_all(&app_dir)?;

    tui::field("appimage", tui::format_path(&appimage_path));

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> tempfile::TempDir {
        kurogane_layout::test_fixtures::tmp_dir()
    }

    fn test_distribution(dir: &Path) -> ResolvedDistribution {
        kurogane_layout::test_fixtures::sample_distribution(dir)
    }

    #[test]
    fn sha256_matches_known_vector() {
        // NIST FIPS 180-4 test vector
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn every_supported_arch_has_a_pinned_digest() {
        for arch in ["x86_64", "aarch64"] {
            let digest = expected_digest(arch).expect("supported arch must be pinned");
            assert_eq!(digest.len(), 64, "a SHA-256 digest is 64 hex characters");
            assert!(digest.chars().all(|c| c.is_ascii_hexdigit()));
        }
    }

    #[test]
    fn unpinned_arch_is_rejected_rather_than_downloaded() {
        let err = expected_digest("riscv64").unwrap_err();

        assert!(
            err.to_string().contains("no pinned linuxdeploy digest"),
            "an unpinned architecture must fail loudly, got: {err}"
        );
    }

    #[test]
    fn apprun_targets_bundle_executable() {
        let content = generate_apprun("custom-name", "custom-bin");

        assert!(content.contains("exec \"$APPDIR/usr/lib/custom-name/custom-bin\""));
    }

    #[test]
    fn apprun_leaves_library_loading_to_rpath() {
        let content = generate_apprun("myapp", "myapp");
        assert!(
            !content.contains("LD_LIBRARY_PATH"),
            "AppRun must not set LD_LIBRARY_PATH; loading is RPATH-owned"
        );
    }

    #[test]
    fn apprun_has_shell_shebang() {
        let content = generate_apprun("exe", "name");
        assert!(content.starts_with("#!/bin/sh"));
    }

    #[test]
    fn desktop_targets_executable() {
        let content = generate_desktop("custom-name", "custom-bin", "2.0.0", &[], false);
        assert!(content.contains("Exec=custom-bin"));
    }

    #[test]
    fn desktop_uses_application_name() {
        let content = generate_desktop("custom-name", "custom-bin", "2.0.0", &[], false);
        assert!(content.contains("Name=custom-name"));
    }

    #[test]
    fn desktop_contains_application_version() {
        let content = generate_desktop("myapp", "myapp", "1.0.0", &[], false);
        assert!(content.contains("X-AppImage-Version=1.0.0"));
        // Spec version, not app version; appimagetool validates this key
        assert!(content.contains("Version=1.0\n"));
    }

    #[test]
    fn desktop_entry_is_valid_format() {
        let content = generate_desktop("myapp", "myapp", "1.0.0", &[], false);
        assert!(content.starts_with("[Desktop Entry]"));
        assert!(content.contains("Type=Application"));
        assert!(content.contains("Terminal=false"));
    }

    #[test]
    fn desktop_defaults_match_historical_output() {
        let content = generate_desktop("myapp", "myapp", "1.0.0", &[], false);

        assert_eq!(
            content,
            "[Desktop Entry]\nType=Application\nName=myapp\nVersion=1.0\nX-AppImage-Version=1.0.0\nExec=myapp\nIcon=myapp\nCategories=Utility;\nTerminal=false\n"
        );
    }

    #[test]
    fn desktop_categories_override_replaces_utility() {
        let categories = vec!["Development".to_string(), "IDE".to_string()];
        let content = generate_desktop("myapp", "myapp", "1.0.0", &categories, false);

        assert!(content.contains("Categories=Development;IDE;"));
        assert!(!content.contains("Utility"));
    }

    #[test]
    fn desktop_terminal_flag_is_configurable() {
        let content = generate_desktop("myapp", "myapp", "1.0.0", &[], true);

        assert!(content.contains("Terminal=true"));
    }

    #[test]
    fn appdir_uses_configured_icon() {
        let dir = tmp();
        let mut dist = test_distribution(dir.path());
        let icon_src = dir.path().join("brand.png");
        fs::write(&icon_src, b"png-bytes").unwrap();
        dist.metadata.icon = Some(icon_src);

        let app_dir = dir.path().join("appdir");

        build_appdir(&dist, &app_dir, &PackagingConfig::default()).unwrap();

        let installed = app_dir.join("usr/share/icons/hicolor/256x256/apps/myapp.png");
        assert_eq!(
            fs::read(&installed).unwrap(),
            b"png-bytes",
            "configured icon must replace the placeholder bytes"
        );
    }

    #[test]
    fn appdir_missing_configured_icon_is_rejected() {
        let dir = tmp();
        let mut dist = test_distribution(dir.path());
        dist.metadata.icon = Some(dir.path().join("nonexistent.png"));

        let app_dir = dir.path().join("appdir");

        let err = build_appdir(&dist, &app_dir, &PackagingConfig::default()).unwrap_err();
        assert!(
            err.to_string().contains("configured icon not found"),
            "expected actionable icon error, got: {err}"
        );
    }

    #[test]
    fn appdir_desktop_reflects_linux_config() {
        let dir = tmp();
        let dist = test_distribution(dir.path());

        let config = PackagingConfig {
            linux: kurogane_layout::LinuxPackagingConfig {
                categories: Some(vec!["Development".into()]),
                terminal: Some(true),
            },
            ..Default::default()
        };
        let app_dir = dir.path().join("appdir");

        build_appdir(&dist, &app_dir, &config).unwrap();

        let desktop =
            fs::read_to_string(app_dir.join("usr/share/applications/myapp.desktop")).unwrap();
        assert!(desktop.contains("Categories=Development;"));
        assert!(desktop.contains("Terminal=true"));
    }

    #[test]
    fn appdir_contains_canonical_bundle() {
        let dir = tmp();
        let dist = test_distribution(dir.path());
        let app_dir = dir.path().join("appdir");

        build_appdir(&dist, &app_dir, &PackagingConfig::default()).unwrap();

        let bundle = app_dir.join("usr/lib/myapp");

        assert!(bundle.join("runtime/myapp").exists());
        assert!(bundle.join("runtime/cef/libcef.so").exists());
        assert!(bundle.join("content/index.html").exists());
        assert!(bundle.join("myapp").exists());
    }

    #[test]
    fn appdir_contains_appimage_metadata() {
        let dir = tmp();
        let dist = test_distribution(dir.path());
        let app_dir = dir.path().join("appdir");

        build_appdir(&dist, &app_dir, &PackagingConfig::default()).unwrap();

        assert!(app_dir.join("AppRun").exists());
        assert!(
            app_dir
                .join("usr/share/applications/myapp.desktop")
                .exists()
        );
        assert!(
            app_dir
                .join("usr/share/icons/hicolor/256x256/apps/myapp.png")
                .exists()
        );
    }

    #[test]
    fn appdir_bundle_is_verified_by_package_directory() {
        let dir = tmp();
        let dist = test_distribution(dir.path());
        let app_dir = dir.path().join("appdir");

        let sabotaged = ResolvedDistribution {
            metadata: dist.metadata.clone(),
            executable: dist.executable.clone(),
            frontend: dist.frontend.clone(),
            cef_runtime: {
                let bad = dir.path().join("bad-cef");
                fs::create_dir_all(&bad).unwrap();
                fs::write(bad.join("libcef.so"), "").unwrap();
                bad
            },
            extra_resources: Vec::new(),
        };

        assert!(
            build_appdir(&sabotaged, &app_dir, &PackagingConfig::default()).is_err(),
            "incomplete CEF runtime must be rejected before imaging"
        );
    }

    #[test]
    fn appdir_no_frontend_does_not_create_content() {
        let dir = tmp();
        let mut dist = test_distribution(dir.path());
        dist.frontend = None;
        let app_dir = dir.path().join("appdir");

        build_appdir(&dist, &app_dir, &PackagingConfig::default()).unwrap();

        assert!(!app_dir.join("usr/lib/myapp/content").exists());
        assert!(app_dir.join("AppRun").exists());
        assert!(app_dir.join("usr/lib/myapp/runtime/myapp").exists());
    }

    #[test]
    fn appdir_extra_resources_land_in_bundle_root() {
        let dir = tmp();
        let mut dist = test_distribution(dir.path());
        let res = dir.path().join("extra.txt");
        fs::write(&res, "resource data").unwrap();
        dist.extra_resources
            .push(kurogane_layout::ResolvedResource {
                source: res.clone(),
                destination: "extra.txt".into(),
            });

        let app_dir = dir.path().join("appdir");
        build_appdir(&dist, &app_dir, &PackagingConfig::default()).unwrap();

        let dest = app_dir.join("usr/lib/myapp/extra.txt");
        assert!(dest.exists(), "extra resource should be in bundle root");
    }
}

//! Assembles macOS .app bundles.
//!
//! CEF is placed under `Contents/Frameworks`, frontend and extra resources
//! under `Contents/Resources` and the executable under `Contents/MacOS`.
//!
//! The CEF framework is signed before the enclosing app.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use anyhow::{Context, Result, bail};
use kurogane_layout::{MACOS_FRAMEWORK, validate_cef_runtime};
use crate::distribution::{AppMetadata, Executable, copy_dir};
use crate::install::NOTICES;
use crate::macos_settings::MacosSettings;
use crate::plist::Dict;
use crate::signing::{SignConfig, sign_app_bundle};

use crate::tui;

/// Frontend resources live under `Contents/Resources/content`.
const CONTENT_DIR: &str = "content";

/// Returns `Contents/MacOS`.
fn macos_dir(app_dir: &Path) -> std::path::PathBuf {
    app_dir.join("Contents").join("MacOS")
}

/// Returns `Contents/Frameworks`.
fn frameworks_dir(app_dir: &Path) -> std::path::PathBuf {
    app_dir.join("Contents").join("Frameworks")
}

/// Folder of `Contents/Resources` holding CEF's licence and credits.
const CEF_NOTICES_DIR: &str = "Chromium Embedded Framework";

/// Returns `Contents/Resources`.
fn resources_dir(app_dir: &Path) -> std::path::PathBuf {
    app_dir.join("Contents").join("Resources")
}

/// The bundle identifier for an application, configured or derived.
fn bundle_identifier(meta: &AppMetadata) -> String {
    meta.identifier
        .clone()
        .unwrap_or_else(|| default_identifier(&meta.name))
}

/// The keys every `Info.plist` of the bundle carries, the application's and
/// each helper's: its identity, and what `[macos]` declares.
fn bundle_plist(
    name: &str,
    identifier: &str,
    executable: &str,
    version: &str,
    macos: &MacosSettings,
) -> Dict {
    let mut plist = Dict::new();
    plist
        .set("CFBundleName", name)
        .set("CFBundleDisplayName", name)
        .set("CFBundleIdentifier", identifier)
        .set("CFBundleExecutable", executable)
        .set("CFBundlePackageType", "APPL")
        .set("CFBundleVersion", version)
        .set("CFBundleShortVersionString", version)
        .set("NSHighResolutionCapable", true);
    macos.declare(&mut plist);
    plist
}

/// Writes `plist` as `<bundle>/Contents/Info.plist`.
fn write_plist(bundle: &Path, plist: &Dict) -> Result<()> {
    let path = bundle.join("Contents").join("Info.plist");
    let xml = plist
        .to_xml()
        .with_context(|| format!("failed to write {}", path.display()))?;
    fs::write(&path, xml).with_context(|| format!("failed to write {}", path.display()))
}

/// Generates `Contents/Info.plist` for the bundle.
fn write_info_plist(
    app_dir: &Path,
    meta: &AppMetadata,
    exe_name: &str,
    icon: bool,
    macos: &MacosSettings,
) -> Result<()> {
    let mut plist = bundle_plist(
        &meta.name,
        &bundle_identifier(meta),
        exe_name,
        &meta.version,
        macos,
    );

    // Reference the icon only when one was actually installed;
    // a dangling CFBundleIconFile leaves the app with no icon at all
    if icon {
        plist.set("CFBundleIconFile", "AppIcon");
    }
    plist
        .set("LSApplicationCategoryType", macos.category.as_str())
        .set("NSSupportsAutomaticGraphicsSwitching", true);

    write_plist(app_dir, &plist)
}

/// Returns a fallback bundle identifier for projects without `[app].identifier`.
///
/// Usable for local builds; Developer ID distribution needs an identifier
/// under the signing team's own domain, hence the config key.
///
/// Non-ASCII and non-alphanumeric characters in the name are replaced with `-`.
fn default_identifier(name: &str) -> String {
    let slug: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();

    format!("com.kurogane.{slug}")
}

/// macOS helper bundles required by CEF.
const HELPERS: &[(&str, &str)] = &[
    ("", "helper"),
    (" (GPU)", "helper.gpu"),
    (" (Plugin)", "helper.plugin"),
    (" (Renderer)", "helper.renderer"),
    (" (Alerts)", "helper.alerts"),
];

/// Writes `Contents/Info.plist` for a helper bundle, kept out of the Dock.
fn write_helper_plist(
    helper_app: &Path,
    name: &str,
    identifier: &str,
    version: &str,
    macos: &MacosSettings,
) -> Result<()> {
    let mut plist = bundle_plist(name, identifier, name, version, macos);
    plist.set("LSUIElement", true);
    write_plist(helper_app, &plist)
}

/// Installs the helper bundles into `Contents/Frameworks/`.
fn install_helpers(
    frameworks: &Path,
    executable: &Path,
    app_name: &str,
    identifier: &str,
    version: &str,
    macos: &MacosSettings,
) -> Result<()> {
    for (suffix, id_suffix) in HELPERS {
        let helper_name = format!("{app_name} Helper{suffix}");
        let helper_app = frameworks.join(format!("{helper_name}.app"));
        let helper_macos = macos_dir(&helper_app);

        fs::create_dir_all(&helper_macos)
            .with_context(|| format!("failed to create directory {}", helper_macos.display()))?;

        let helper_exe = helper_macos.join(&helper_name);

        fs::copy(executable, &helper_exe).with_context(|| {
            format!(
                "failed to copy {} to {}",
                executable.display(),
                helper_exe.display()
            )
        })?;
        fs::set_permissions(&helper_exe, fs::Permissions::from_mode(0o755))
            .with_context(|| format!("failed to set permissions on {}", helper_exe.display()))?;

        write_helper_plist(
            &helper_app,
            &helper_name,
            &format!("{identifier}.{id_suffix}"),
            version,
            macos,
        )?;
    }

    Ok(())
}

/// Installs the configured icon as `AppIcon.icns`.
///
/// Returns whether an icon was installed.
fn install_icon(resources: &Path, icon: &Path) -> Result<bool> {
    if !icon.exists() {
        tui::warn(&format!(
            "configured icon '{}' does not exist; bundling without it",
            icon.display()
        ));

        return Ok(false);
    }

    let ext = icon.extension().and_then(|e| e.to_str()).unwrap_or("");
    let is_icns = ext.eq_ignore_ascii_case("icns");

    if is_icns {
        let dest = resources.join("AppIcon.icns");
        fs::copy(icon, &dest)
            .with_context(|| format!("failed to copy {} to {}", icon.display(), dest.display()))?;
        return Ok(true);
    }

    // `sips` converts raster images to `.icns`
    let out = resources.join("AppIcon.icns");
    let status = std::process::Command::new("sips")
        .arg("-s")
        .arg("format")
        .arg("icns")
        .arg(icon)
        .arg("--out")
        .arg(&out)
        .status();

    // An icon failure does not prevent the bundle from being created
    let status = match status {
        Ok(status) => status,

        Err(e) => {
            let _ = fs::remove_file(&out);
            tui::warn(&format!(
                "could not run sips ({e}); bundling without an icon"
            ));

            return Ok(false);
        }
    };

    if !status.success() {
        let _ = fs::remove_file(&out);
        tui::warn(&format!(
            "sips could not convert icon '{}' to .icns; bundling without it",
            icon.display()
        ));
        return Ok(false);
    }

    Ok(true)
}

/// Builds and optionally signs a macOS `.app` bundle declaring what `macos`
/// says.
pub fn build(
    dist: &crate::distribution::ResolvedDistribution,
    output_dir: &Path,
    macos: &MacosSettings,
    sign_config: Option<&SignConfig>,
) -> Result<std::path::PathBuf> {
    let app_name = dist.metadata.name.clone();
    let app_dir = output_dir.join(format!("{app_name}.app"));

    if app_dir.exists() {
        fs::remove_dir_all(&app_dir)
            .with_context(|| format!("failed to remove directory {}", app_dir.display()))?;
    }

    for dir in [
        macos_dir(&app_dir),
        frameworks_dir(&app_dir),
        resources_dir(&app_dir),
    ] {
        fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create directory {}", dir.display()))?;
    }

    let exe_name = dist.metadata.exe_name.clone();
    let exe_dest = macos_dir(&app_dir).join(&exe_name);

    // Main executable; CEF's sandbox bootstrap is a Windows shape
    let Executable::Application(executable) = &dist.executable else {
        bail!("a macOS app bundle starts the application itself, not CEF's Windows bootstrap");
    };

    fs::copy(executable, &exe_dest).with_context(|| {
        format!(
            "failed to copy {} to {}",
            executable.display(),
            exe_dest.display()
        )
    })?;
    fs::set_permissions(&exe_dest, fs::Permissions::from_mode(0o755))
        .with_context(|| format!("failed to set permissions on {}", exe_dest.display()))?;

    // CEF framework
    let framework_src = dist.cef_runtime.join(MACOS_FRAMEWORK);
    if !framework_src.exists() {
        bail!(
            "CEF runtime at {} does not contain the {MACOS_FRAMEWORK}",
            dist.cef_runtime.display()
        );
    }
    let framework_dest = frameworks_dir(&app_dir).join(MACOS_FRAMEWORK);
    copy_dir(&framework_src, &framework_dest).with_context(|| {
        format!(
            "failed to copy the CEF framework to {}",
            framework_dest.display()
        )
    })?;

    // Validate the placed framework
    validate_cef_runtime(&frameworks_dir(&app_dir))?;

    // CEF's licence and credits, outside the framework its signature covers
    let notices = resources_dir(&app_dir).join(CEF_NOTICES_DIR);
    fs::create_dir_all(&notices)
        .with_context(|| format!("failed to create directory {}", notices.display()))?;
    for notice in NOTICES {
        let (src, dest) = (dist.cef_runtime.join(notice), notices.join(notice));
        fs::copy(&src, &dest)
            .with_context(|| format!("failed to copy {} to {}", src.display(), dest.display()))?;
    }

    // Subprocess helpers, beside the framework. Without these macOS launches no
    // renderer and the packaged app shows a blank window.
    install_helpers(
        &frameworks_dir(&app_dir),
        &exe_dest,
        &app_name,
        &bundle_identifier(&dist.metadata),
        &dist.metadata.version,
        macos,
    )?;

    // Icon, before the plist so it can reference it only when present
    let icon = match &dist.metadata.icon {
        Some(icon) => install_icon(&resources_dir(&app_dir), icon)?,
        None => false,
    };

    write_info_plist(&app_dir, &dist.metadata, &exe_name, icon, macos)?;

    // Frontend resources
    if let Some(frontend) = &dist.frontend {
        let content = resources_dir(&app_dir).join(CONTENT_DIR);
        copy_dir(frontend, &content)
            .with_context(|| format!("failed to copy the frontend to {}", content.display()))?;
    }

    // Extra resources
    for resource in &dist.extra_resources {
        let dest = resources_dir(&app_dir).join(&resource.destination);
        if resource.source.is_dir() {
            copy_dir(&resource.source, &dest).with_context(|| {
                format!(
                    "failed to copy {} to {}",
                    resource.source.display(),
                    dest.display()
                )
            })?;
        } else {
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("failed to create directory {}", parent.display()))?;
            }
            fs::copy(&resource.source, &dest).with_context(|| {
                format!(
                    "failed to copy {} to {}",
                    resource.source.display(),
                    dest.display()
                )
            })?;
        }
    }

    // Signing (optional); Entitlements are a signing input, not an app
    // resource, so the plist is written beside the bundle and removed again
    // rather than sealed into Contents/Resources
    if let Some(config) = sign_config {
        let entitlements = output_dir.join(format!("{app_name}.entitlements.plist"));
        let xml = macos
            .entitlements()
            .to_xml()
            .with_context(|| format!("failed to write {}", entitlements.display()))?;
        fs::write(&entitlements, xml)
            .with_context(|| format!("failed to write {}", entitlements.display()))?;

        let signed = sign_app_bundle(&app_dir, config, Some(&entitlements));
        let _ = fs::remove_file(&entitlements);
        signed?;

        tui::field("signed", format!("{app_name}.app"));
    }

    Ok(app_dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MacosPackagingConfig;
    use crate::distribution::{AppMetadata, Executable, ResolvedDistribution, ResolvedResource};

    /// What an empty `[macos]` declares.
    fn settings() -> MacosSettings {
        MacosSettings::resolve(&MacosPackagingConfig::default()).unwrap()
    }

    fn sample_metadata() -> AppMetadata {
        AppMetadata {
            name: "MyApp".to_string(),
            version: "1.0.0".to_string(),
            exe_name: "myapp".to_string(),
            identifier: None,
            publisher: None,
            description: None,
            copyright: None,
            icon: None,
        }
    }

    fn write_executable(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"mach-o").unwrap();
    }

    // Minimal CEF framework shape required by `validate_cef_runtime`, with
    // the notices an installation carries
    fn framework_fixture(root: &Path) {
        fs::create_dir_all(root).unwrap();
        for notice in NOTICES {
            fs::write(root.join(notice), b"notice").unwrap();
        }
        let fw = root.join("Chromium Embedded Framework.framework");
        let resources = fw.join("Resources");
        fs::create_dir_all(&resources).unwrap();
        fs::write(fw.join("Chromium Embedded Framework"), b"binary").unwrap();
        fs::write(resources.join("icudtl.dat"), b"").unwrap();
        fs::create_dir_all(resources.join("en.lproj")).unwrap();
        fs::write(resources.join("v8_context_snapshot.arm64.bin"), b"").unwrap();
    }

    #[test]
    fn writes_contents_macos_frameworks_resources() {
        let dir = tempfile::tempdir().unwrap();
        let cef = dir.path().join("cef");
        framework_fixture(&cef);

        let exe = dir.path().join("target").join("release").join("myapp");
        write_executable(&exe);

        let dist = ResolvedDistribution {
            metadata: sample_metadata(),
            executable: Executable::Application(exe),
            frontend: None,
            cef_runtime: cef,
            extra_resources: Vec::new(),
        };

        let output = dir.path().join("dist");
        let app_dir = build(&dist, &output, &settings(), None).unwrap();

        // Avoid asserting on platform tooling unavailable in every test environment
        assert!(
            app_dir
                .join("Contents")
                .join("MacOS")
                .join("myapp")
                .exists()
        );
        assert!(
            app_dir
                .join("Contents")
                .join("Frameworks")
                .join("Chromium Embedded Framework.framework")
                .join("Chromium Embedded Framework")
                .exists()
        );
        assert!(app_dir.join("Contents").join("Info.plist").exists());
        for notice in NOTICES {
            assert!(
                resources_dir(&app_dir)
                    .join(CEF_NOTICES_DIR)
                    .join(notice)
                    .is_file(),
                "the bundle passes on CEF's {notice}"
            );
        }
    }

    #[test]
    fn helpers_are_installed_beside_the_framework() {
        let dir = tempfile::tempdir().unwrap();
        let cef = dir.path().join("cef");
        framework_fixture(&cef);
        let exe = dir.path().join("target").join("release").join("myapp");
        write_executable(&exe);

        let dist = ResolvedDistribution {
            metadata: sample_metadata(),
            executable: Executable::Application(exe),
            frontend: None,
            cef_runtime: cef,
            extra_resources: Vec::new(),
        };

        let output = dir.path().join("dist");
        let app_dir = build(&dist, &output, &settings(), None).unwrap();
        let frameworks = app_dir.join("Contents").join("Frameworks");

        for (suffix, id_suffix) in HELPERS {
            let name = format!("MyApp Helper{suffix}");
            let helper = frameworks.join(format!("{name}.app"));

            assert!(
                helper.join("Contents").join("MacOS").join(&name).is_file(),
                "{name} must carry the application binary"
            );

            let plist = fs::read_to_string(helper.join("Contents").join("Info.plist")).unwrap();

            // Keep helpers out of the Dock
            assert!(plist.contains("<key>LSUIElement</key>"), "{name}: {plist}");
            assert!(
                plist.contains(&format!("<string>com.kurogane.myapp.{id_suffix}</string>")),
                "{name} needs its own identifier: {plist}"
            );
        }
    }

    #[test]
    fn every_plist_of_the_bundle_declares_the_macos_and_the_devices() {
        let dir = tempfile::tempdir().unwrap();
        let cef = dir.path().join("cef");
        framework_fixture(&cef);
        let exe = dir.path().join("target").join("release").join("myapp");
        write_executable(&exe);
        let dist = ResolvedDistribution {
            metadata: sample_metadata(),
            executable: Executable::Application(exe),
            frontend: None,
            cef_runtime: cef,
            extra_resources: Vec::new(),
        };
        let mut macos = MacosPackagingConfig {
            minimum_system_version: Some("13.0".into()),
            category: Some("public.app-category.video".into()),
            ..MacosPackagingConfig::default()
        };
        macos.privacy.camera = Some("Calls use your camera.".into());

        let output = dir.path().join("dist");
        let app_dir = build(
            &dist,
            &output,
            &MacosSettings::resolve(&macos).unwrap(),
            None,
        )
        .unwrap();

        let frameworks = app_dir.join("Contents").join("Frameworks");
        let plists = std::iter::once(app_dir.clone()).chain(
            HELPERS
                .iter()
                .map(|(suffix, _)| frameworks.join(format!("MyApp Helper{suffix}.app"))),
        );
        for bundle in plists {
            let plist = fs::read_to_string(bundle.join("Contents/Info.plist")).unwrap();
            for expected in [
                "<key>LSMinimumSystemVersion</key>\n    <string>13.0</string>",
                "<key>NSCameraUsageDescription</key>\n    <string>Calls use your camera.</string>",
                "<key>NSMicrophoneUsageDescription</key>",
            ] {
                assert!(plist.contains(expected), "{}: {plist}", bundle.display());
            }
        }
        let app = fs::read_to_string(app_dir.join("Contents/Info.plist")).unwrap();
        assert!(app.contains("<string>public.app-category.video</string>"));
    }

    #[test]
    fn info_plist_contains_required_keys() {
        let dir = tempfile::tempdir().unwrap();
        let contents = dir.path().join("Contents");
        fs::create_dir_all(&contents).unwrap();

        write_info_plist(dir.path(), &sample_metadata(), "myapp", false, &settings()).unwrap();

        let plist = fs::read_to_string(contents.join("Info.plist")).unwrap();
        assert!(plist.contains("<key>CFBundleExecutable</key>"));
        assert!(plist.contains("<string>myapp</string>"));
        assert!(plist.contains("<key>CFBundleIdentifier</key>"));
        assert!(plist.contains("<key>CFBundlePackageType</key>"));
        assert!(plist.contains("<string>APPL</string>"));
    }

    #[test]
    fn build_without_signing_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let cef = dir.path().join("cef");
        framework_fixture(&cef);
        let exe = dir.path().join("target").join("release").join("myapp");
        write_executable(&exe);

        let dist = ResolvedDistribution {
            metadata: sample_metadata(),
            executable: Executable::Application(exe),
            frontend: None,
            cef_runtime: cef,
            extra_resources: Vec::new(),
        };

        let output = dir.path().join("dist");
        let app_dir = build(&dist, &output, &settings(), None).unwrap();
        assert!(app_dir.exists());
    }

    #[test]
    fn frontend_lands_in_resources_content() {
        let dir = tempfile::tempdir().unwrap();
        let cef = dir.path().join("cef");
        framework_fixture(&cef);
        let exe = dir.path().join("target").join("release").join("myapp");
        write_executable(&exe);

        let frontend = dir.path().join("web").join("dist");
        fs::create_dir_all(&frontend).unwrap();
        fs::write(frontend.join("index.html"), b"<html></html>").unwrap();

        let dist = ResolvedDistribution {
            metadata: sample_metadata(),
            executable: Executable::Application(exe),
            frontend: Some(frontend),
            cef_runtime: cef,
            extra_resources: Vec::new(),
        };

        let output = dir.path().join("dist");
        let app_dir = build(&dist, &output, &settings(), None).unwrap();

        // Contents/Resources is the bundle resource root
        assert!(
            app_dir
                .join("Contents/Resources/content/index.html")
                .exists(),
            "frontend must be delivered at Contents/Resources/content/"
        );
    }

    #[test]
    fn delivered_bundle_has_no_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let cef = dir.path().join("cef");
        framework_fixture(&cef);

        // Bundled files must not depend on symlinks in the build tree
        let libraries = cef
            .join("Chromium Embedded Framework.framework")
            .join("Libraries");
        fs::create_dir_all(&libraries).unwrap();
        let real = dir.path().join("libGLESv2.dylib");
        fs::write(&real, b"dylib").unwrap();
        std::os::unix::fs::symlink(&real, libraries.join("libGLESv2.dylib")).unwrap();

        let exe = dir.path().join("target").join("release").join("myapp");
        write_executable(&exe);

        let dist = ResolvedDistribution {
            metadata: sample_metadata(),
            executable: Executable::Application(exe),
            frontend: None,
            cef_runtime: cef,
            extra_resources: Vec::new(),
        };

        let output = dir.path().join("dist");
        let app_dir = build(&dist, &output, &settings(), None).unwrap();

        let mut links = Vec::new();
        let mut stack = vec![app_dir.clone()];
        while let Some(path) = stack.pop() {
            for entry in fs::read_dir(&path).unwrap() {
                let entry = entry.unwrap();
                let meta = entry.path().symlink_metadata().unwrap();
                if meta.file_type().is_symlink() {
                    links.push(entry.path());
                } else if meta.is_dir() {
                    stack.push(entry.path());
                }
            }
        }

        assert!(
            links.is_empty(),
            "delivered bundle contains symlinks: {links:?}"
        );
        assert_eq!(
            fs::read(app_dir.join(
                "Contents/Frameworks/Chromium Embedded Framework.framework/Libraries/libGLESv2.dylib"
            ))
            .unwrap(),
            b"dylib",
            "the symlink must be materialized with its contents"
        );
    }

    #[test]
    fn info_plist_references_the_icon_only_when_one_is_installed() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("Contents")).unwrap();

        write_info_plist(dir.path(), &sample_metadata(), "myapp", false, &settings()).unwrap();
        let without = fs::read_to_string(dir.path().join("Contents/Info.plist")).unwrap();
        assert!(!without.contains("CFBundleIconFile"));

        write_info_plist(dir.path(), &sample_metadata(), "myapp", true, &settings()).unwrap();
        let with = fs::read_to_string(dir.path().join("Contents/Info.plist")).unwrap();
        assert!(with.contains("<key>CFBundleIconFile</key>"));
        assert!(with.contains("<string>AppIcon</string>"));
    }

    #[test]
    fn configured_identifier_overrides_the_default() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("Contents")).unwrap();

        let mut meta = sample_metadata();
        meta.identifier = Some("com.example.myapp".to_string());
        write_info_plist(dir.path(), &meta, "myapp", false, &settings()).unwrap();

        let plist = fs::read_to_string(dir.path().join("Contents/Info.plist")).unwrap();
        assert!(plist.contains("<string>com.example.myapp</string>"));
        assert!(!plist.contains("com.kurogane."));
    }

    #[test]
    fn default_identifier_is_a_reverse_dns_slug() {
        assert_eq!(default_identifier("My App"), "com.kurogane.my-app");
    }

    #[test]
    fn default_identifier_is_ascii_only() {
        // Bundle identifiers must be ASCII
        let identifier = default_identifier("Café");

        assert_eq!(identifier, "com.kurogane.caf-");
        assert!(identifier.is_ascii(), "{identifier} must be ASCII");
    }

    #[test]
    fn info_plist_escapes_configured_values() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("Contents")).unwrap();

        let mut meta = sample_metadata();
        meta.name = "Ben & Jerry <Ltd>".to_string();

        write_info_plist(dir.path(), &meta, "myapp", false, &settings()).unwrap();

        let plist = fs::read_to_string(dir.path().join("Contents/Info.plist")).unwrap();

        // Unescaped XML would produce an invalid plist
        assert!(plist.contains("<string>Ben &amp; Jerry &lt;Ltd&gt;</string>"));
        assert!(!plist.contains("Ben & Jerry"));
    }

    #[test]
    fn resources_land_in_resources_directory() {
        let dir = tempfile::tempdir().unwrap();
        let cef = dir.path().join("cef");
        framework_fixture(&cef);
        let exe = dir.path().join("target").join("release").join("myapp");
        write_executable(&exe);
        let asset = dir.path().join("asset.txt");
        fs::write(&asset, b"data").unwrap();

        let dist = ResolvedDistribution {
            metadata: sample_metadata(),
            executable: Executable::Application(exe),
            frontend: None,
            cef_runtime: cef,
            extra_resources: vec![ResolvedResource {
                source: asset,
                destination: "share/asset.txt".into(),
            }],
        };

        let output = dir.path().join("dist");
        let app_dir = build(&dist, &output, &settings(), None).unwrap();
        assert!(
            app_dir
                .join("Contents")
                .join("Resources")
                .join("share")
                .join("asset.txt")
                .exists()
        );
    }
}

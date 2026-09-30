//! Application build and packaging orchestration.
//!
//! This module resolves application inputs, materializes the canonical
//! distribution, selects the requested package format and coordinates
//! optional signing.

use anyhow::{Result, bail};
use std::env::consts::EXE_SUFFIX;
use std::ffi::OsString;
use std::process::Command;
use cargo_metadata::{MetadataCommand, Package, TargetKind};
use kurogane_layout::{
    AppConfig, AppMetadata, Executable, PackagingConfig, ResolvedDistribution, SignConfig,
    anchor_path, materialize_cef_runtime, resolve_cef_for_bundle,
};

use crate::launch::find_target;
#[cfg(not(target_os = "macos"))]
use kurogane_layout::{package_directory, sign_tree, verify_tree};

use crate::tui;

/// Runs the configured frontend build command.
///
/// The command is interpreted by the platform shell.
fn build_frontend(
    workspace_root: &std::path::Path,
    config: &kurogane_layout::AppConfig,
) -> Result<()> {
    let Some(command) = &config.frontend_build else {
        return Ok(());
    };
    if command.trim().is_empty() {
        bail!("empty frontend-build command");
    }

    tui::step("Building frontend...");

    let status = shell(command).current_dir(workspace_root).status()?;

    if !status.success() {
        bail!("Frontend build failed: {command}");
    }

    Ok(())
}

/// Runs `command` through `sh -c`.
#[cfg(not(target_os = "windows"))]
fn shell(command: &str) -> Command {
    let mut shell = Command::new("sh");
    shell.arg("-c").arg(command);
    shell
}

/// Runs `command` through `cmd`.
#[cfg(target_os = "windows")]
fn shell(command: &str) -> Command {
    use std::os::windows::process::CommandExt;

    let mut shell = Command::new("cmd");
    shell.raw_arg(format!("/d /s /c \"{command}\""));
    shell
}

/// Output format for the application bundle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageFormat {
    /// Plain directory bundle (default except on macOS).
    #[cfg(not(target_os = "macos"))]
    Directory,
    /// Linux AppImage.
    #[cfg(target_os = "linux")]
    AppImage,
    /// Windows NSIS installer.
    #[cfg(target_os = "windows")]
    Nsis,
    /// macOS `.app/.dmg` bundle.
    #[cfg(target_os = "macos")]
    AppBundle,
}

/// Default `--format`: the platform's native distributable.
#[cfg(target_os = "macos")]
pub(crate) const DEFAULT_FORMAT: &str = "app";

/// Default `--format`: the platform's native distributable.
#[cfg(not(target_os = "macos"))]
pub(crate) const DEFAULT_FORMAT: &str = "dir";

impl PackageFormat {
    /// The formats available on the current build platform.
    fn available() -> Vec<&'static str> {
        [
            #[cfg(not(target_os = "macos"))]
            "dir; directory",
            #[cfg(target_os = "linux")]
            "appimage",
            #[cfg(target_os = "windows")]
            "nsis",
            #[cfg(target_os = "macos")]
            "app",
        ]
        .to_vec()
    }

    pub fn from_str(s: &str) -> Result<Self> {
        match s {
            #[cfg(not(target_os = "macos"))]
            "dir" | "directory" => Ok(PackageFormat::Directory),
            #[cfg(target_os = "macos")]
            "dir" | "directory" => bail!(
                "directory format is not supported on macOS; \
                 use '--format app', which produces a .app bundle and a .dmg"
            ),
            #[cfg(target_os = "linux")]
            "appimage" => Ok(PackageFormat::AppImage),
            #[cfg(target_os = "windows")]
            "nsis" => Ok(PackageFormat::Nsis),
            #[cfg(target_os = "macos")]
            "app" | "appbundle" => Ok(PackageFormat::AppBundle),
            _ => bail!(
                "unsupported format: {s}\n\n\
                 Available on this platform: {}\n\
                 (appimage requires Linux; nsis requires Windows; app requires macOS)",
                Self::available().join(", ")
            ),
        }
    }
}

/// Resolves configured bundle resources against the project root.
///
/// Only the source is anchored; the destination is bundle-relative.
fn resolve_resources(
    project_root: &std::path::Path,
    configured: &[kurogane_layout::ResourceConfig],
) -> Result<Vec<kurogane_layout::ResolvedResource>> {
    configured
        .iter()
        .map(|resource| {
            let mut resolved = resource.to_resolved()?;
            resolved.source = anchor_path(project_root, &resolved.source);
            Ok(resolved)
        })
        .collect()
}

/// Signs and verifies all signable artifacts in a staged bundle.
///
/// Warns when the bundle contains no signable artifacts.
#[cfg(not(target_os = "macos"))]
pub(crate) fn sign_and_verify_tree(root: &std::path::Path, config: &SignConfig) -> Result<()> {
    let signed = sign_tree(root, config)?;

    if signed == 0 {
        tui::warn("No signable artifacts found in the bundle");
        return Ok(());
    }

    tui::field("signed", format!("{signed} file(s)"));

    let verified = verify_tree(root, config)?;
    tui::field("verified", format!("{verified} file(s)"));

    Ok(())
}

/// Resolves the signing policy for a packaging operation.
fn resolve_sign_config(
    sign_requested: bool,
    config: &PackagingConfig,
    project_root: &std::path::Path,
) -> Result<Option<SignConfig>> {
    if !sign_requested {
        return Ok(None);
    }

    let mut resolved = SignConfig::from_file_config(&config.signing)?.ok_or_else(|| {
        anyhow::anyhow!(
            "--sign requested but no usable [signing] configuration found in {} \
             (set `certificate`, `certificate-thumbprint`, `certificate-identity` \
             or `custom-command`)",
            kurogane_layout::CONFIG_FILE_NAME
        )
    })?;

    // Configured certificate path is project-relative
    if let Some(kurogane_layout::CertificateSource::File { path, password_env }) =
        resolved.certificate
    {
        resolved.certificate = Some(kurogane_layout::CertificateSource::File {
            path: anchor_path(project_root, &path),
            password_env,
        });
    }

    Ok(Some(resolved))
}

/// Builds the application in the requested profile.
///
/// Returns what the bundle starts and the name it is installed under.
fn build_executable(
    cef: &std::path::Path,
    pkg: &Package,
    app: &AppConfig,
    debug: bool,
    target_dir: &std::path::Path,
) -> Result<(Executable, String)> {
    let profile: &[&str] = if debug {
        &["--features", "kurogane/debug"]
    } else {
        &["--release"]
    };

    if crate::sandbox::uses_bootstrap(app) {
        // The bootstrap is CEF's; only the application's library is built
        let profile: Vec<OsString> = profile.iter().map(OsString::from).collect();
        let library = crate::sandbox::build_library(cef, pkg, &profile)?;
        let exe_name = format!("{}{EXE_SUFFIX}", crate::sandbox::app_name(pkg)?);

        return Ok((crate::sandbox::bundled(cef, library)?, exe_name));
    }

    let status = crate::launch::cargo_command(cef, "build")?
        .args(profile)
        .status()?;

    if !status.success() {
        bail!("Build failed");
    }

    let target = find_target(pkg, TargetKind::Bin)
        .ok_or_else(|| anyhow::anyhow!("No binary target found"))?;

    let exe_name = format!("{}{EXE_SUFFIX}", target.name);
    let executable = target_dir.join(&exe_name);

    if !executable.exists() {
        bail!("Executable not found: {}", executable.display());
    }

    Ok((Executable::Application(executable), exe_name))
}

/// Build the application in the requested profile and package it.
pub fn run(debug: bool, format: PackageFormat, sign: bool) -> Result<()> {
    tui::section("Kurogane Bundle");

    // Declarative packaging configuration; defaults when absent
    let metadata = MetadataCommand::new().exec()?;

    let packaging_config = PackagingConfig::load(metadata.workspace_root.as_std_path())?;

    // Build frontend before cargo build
    build_frontend(metadata.workspace_root.as_std_path(), &packaging_config.app)?;

    tui::step("Resolving CEF runtime...");

    let cef = resolve_cef_for_bundle(env!("KUROGANE_CEF_VERSION"))?;

    match cef.source {
        kurogane_layout::CefSource::ManagedCache => {
            if let Some(p) = &cef.provenance {
                tui::field("cef", format!("{} (managed)", p.cef_version));
            }
        }
        kurogane_layout::CefSource::EnvironmentOverride => {
            if let Some(p) = &cef.provenance {
                tui::field("cef", format!("{} (CEF_PATH)", p.cef_version));
            }
        }
    }

    let pkg = metadata
        .root_package()
        .ok_or_else(|| anyhow::anyhow!("No root package"))?;

    let profile = if debug { "debug" } else { "release" };
    let kurogane_target = crate::launch::target_dir_in(metadata.target_directory.as_std_path());
    let target_dir = kurogane_target.join(profile);

    tui::step(&format!("Building {profile}..."));

    let (executable, exe_name) =
        build_executable(&cef.root, pkg, &packaging_config.app, debug, &target_dir)?;

    // Resolve distribution contents
    tui::step("Resolving distribution...");

    // Materialize the runnable runtime
    let runtime_version = cef
        .provenance
        .as_ref()
        .map(|p| p.cef_version.clone())
        .unwrap_or_else(|| env!("KUROGANE_CEF_VERSION").to_string());

    let runtime_dir = kurogane_target.join("cef-runtime").join(&runtime_version);

    let cef_runtime = materialize_cef_runtime(&cef.root, &runtime_dir)?;

    // Configured paths are relative to the project root
    let project_root = metadata.workspace_root.as_std_path();

    let frontend = match &packaging_config.app.frontend_dist {
        Some(path) => {
            let path = anchor_path(project_root, path);
            if path.exists() {
                Some(path)
            } else {
                tui::warn(&format!(
                    "Configured frontend distribution '{}' does not exist. \
                     Build it first (e.g. the frontend-build command).",
                    path.display()
                ));
                None
            }
        }
        None => {
            tui::info("No frontend-dist configured in kurogane.toml");
            None
        }
    };

    let extra_resources = resolve_resources(project_root, &packaging_config.bundle.resources)?;

    let dist = ResolvedDistribution {
        metadata: AppMetadata {
            name: packaging_config
                .app
                .name
                .clone()
                .unwrap_or_else(|| pkg.name.to_string()),
            version: pkg.version.to_string(),
            exe_name,
            identifier: packaging_config.app.identifier.clone(),
            publisher: packaging_config.app.publisher.clone(),
            description: packaging_config.app.description.clone(),
            copyright: packaging_config.app.copyright.clone(),
            icon: packaging_config
                .app
                .icon
                .as_ref()
                .map(|icon| anchor_path(project_root, icon)),
        },
        executable,
        frontend,
        cef_runtime,
        extra_resources,
    };

    dist.validate()
        .map_err(|e| anyhow::anyhow!("distribution validation failed: {e}"))?;

    tui::field("binary", tui::format_path(dist.executable.binary()));

    if let Some(library) = dist.executable.library() {
        tui::field("library", tui::format_path(library));
    }
    tui::field("format", format!("{format:?}"));

    // Package the distribution
    tui::step("Packaging...");

    let output_dir = project_root.join("dist");
    let sign_config = resolve_sign_config(sign, &packaging_config, project_root)?;

    match format {
        #[cfg(not(target_os = "macos"))]
        PackageFormat::Directory => {
            // The canonical bundle is the artifact; sign it in place
            let output = package_directory(&dist, &output_dir)?;

            if let Some(config) = &sign_config {
                sign_and_verify_tree(&output, config)?;
            }

            tui::field("output", tui::format_path(&output));
        }

        #[cfg(target_os = "linux")]
        PackageFormat::AppImage => {
            crate::appimage::build(&dist, &output_dir, &packaging_config, sign_config.as_ref())?;
        }

        #[cfg(target_os = "windows")]
        PackageFormat::Nsis => {
            crate::nsis::build(&dist, &output_dir, &packaging_config, sign_config.as_ref())?;
        }

        #[cfg(target_os = "macos")]
        PackageFormat::AppBundle => {
            let app_dir = crate::app_bundle::build(&dist, &output_dir, sign_config.as_ref())?;
            let name = dist.metadata.name.clone();
            crate::dmg::build(&app_dir, &output_dir, &name)?;
            tui::field("output", tui::format_path(&app_dir));
        }
    }

    tui::blank();
    tui::success("Bundle ready");
    tui::field("path", tui::format_path(&output_dir));

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stdout_of(command: &str) -> String {
        let output = shell(command).output().unwrap();
        assert!(output.status.success(), "{command}: {output:?}");
        String::from_utf8(output.stdout).unwrap()
    }

    #[test]
    #[cfg(not(target_os = "windows"))]
    fn frontend_build_runs_through_the_shell() {
        assert_eq!(stdout_of("v='a b' && printf '%s' \"$v\""), "a b");
        assert_eq!(stdout_of("printf one; printf two"), "onetwo");
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn frontend_build_runs_through_cmd() {
        assert_eq!(stdout_of("echo a&& echo \"b c\""), "a\r\n\"b c\"\r\n");
    }

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn parses_directory_aliases() {
        assert!(matches!(
            PackageFormat::from_str("dir"),
            Ok(PackageFormat::Directory)
        ));
        assert!(matches!(
            PackageFormat::from_str("directory"),
            Ok(PackageFormat::Directory)
        ));
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn rejects_the_directory_format_with_a_pointer_to_app() {
        for alias in ["dir", "directory"] {
            let err = PackageFormat::from_str(alias).unwrap_err().to_string();

            assert!(err.contains("not supported on macOS"), "got: {err}");
            assert!(err.contains("--format app"), "got: {err}");
        }

        assert!(PackageFormat::from_str("app").is_ok());
    }

    #[test]
    fn rejects_unsupported_format() {
        let err = PackageFormat::from_str("msi").unwrap_err();

        assert!(err.to_string().contains("unsupported format"));
        assert!(
            err.to_string().contains("Available on this platform"),
            "the error should list what this platform actually supports"
        );
    }

    #[test]
    fn rejects_empty_format() {
        assert!(PackageFormat::from_str("").is_err());
    }

    fn resource(source: &str, destination: Option<&str>) -> kurogane_layout::ResourceConfig {
        kurogane_layout::ResourceConfig {
            source: source.into(),
            destination: destination.map(str::to_owned),
        }
    }

    #[test]
    fn resource_sources_anchor_to_the_project_root() {
        let root = std::path::Path::new("/workspace/app");

        let resolved = resolve_resources(root, &[resource("assets/data", Some("share/data"))])
            .expect("resources should resolve");

        assert_eq!(
            resolved[0].source,
            std::path::PathBuf::from("/workspace/app/assets/data"),
            "a relative source must resolve against the project, not the shell's cwd"
        );
    }

    #[test]
    fn resource_destinations_stay_bundle_relative() {
        let root = std::path::Path::new("/workspace/app");

        let resolved = resolve_resources(
            root,
            &[
                resource("assets/data", Some("share/data")),
                resource("README.md", None),
            ],
        )
        .expect("resources should resolve");

        assert_eq!(
            resolved[0].destination,
            std::path::PathBuf::from("share/data")
        );
        assert_eq!(
            resolved[1].destination,
            std::path::PathBuf::from("README.md"),
            "an omitted destination defaults to the source file name"
        );
        for entry in &resolved {
            assert!(
                entry.destination.is_relative(),
                "anchoring must never leak into the bundle destination"
            );
        }
    }

    #[test]
    fn absolute_resource_sources_are_left_alone() {
        let absolute = if cfg!(windows) {
            r"C:\shared\assets"
        } else {
            "/shared/assets"
        };

        let resolved = resolve_resources(
            std::path::Path::new("/workspace/app"),
            &[resource(absolute, None)],
        )
        .expect("resources should resolve");

        assert_eq!(resolved[0].source, std::path::PathBuf::from(absolute));
    }
}

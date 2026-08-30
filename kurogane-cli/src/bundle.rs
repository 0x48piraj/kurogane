//! Application build and packaging orchestration.
//!
//! This module resolves application inputs, materializes the canonical
//! distribution, selects the requested package format and coordinates
//! optional signing.

use anyhow::{Result, bail};
use std::path::PathBuf;
use std::process::Command;
use cargo_metadata::{MetadataCommand, TargetKind};
use kurogane_layout::{
    AppMetadata, PackagingConfig, ResolvedDistribution, SignConfig, materialize_cef_runtime,
    package_directory, resolve_cef_for_bundle, sign_tree,
};

use crate::tui;

/// Run the frontend build command if configured.
fn build_frontend(
    workspace_root: &std::path::Path,
    config: &kurogane_layout::AppConfig,
) -> Result<()> {
    let Some(command) = &config.frontend_build else {
        return Ok(());
    };

    let package_json = workspace_root.join("package.json");
    if !package_json.exists() {
        return Ok(());
    }

    tui::step("Building frontend...");

    #[cfg(target_os = "windows")]
    let status = Command::new("cmd")
        .args(["/C", command])
        .current_dir(workspace_root)
        .status()?;

    #[cfg(not(target_os = "windows"))]
    let status = {
        let parts: Vec<&str> = command.split_whitespace().collect();
        let (program, args) = parts
            .split_first()
            .ok_or_else(|| anyhow::anyhow!("empty frontend-build command"))?;

        Command::new(*program)
            .args(args)
            .current_dir(workspace_root)
            .status()?
    };

    if !status.success() {
        bail!("Frontend build failed: {command}");
    }

    Ok(())
}

/// Output format for the application bundle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageFormat {
    /// Plain directory bundle (default).
    Directory,
    /// Linux AppImage.
    #[cfg(target_os = "linux")]
    AppImage,
    /// Windows NSIS installer.
    #[cfg(target_os = "windows")]
    Nsis,
}

impl PackageFormat {
    pub fn from_str(s: &str) -> Result<Self> {
        match s {
            "dir" | "directory" => Ok(PackageFormat::Directory),
            #[cfg(target_os = "linux")]
            "appimage" => Ok(PackageFormat::AppImage),
            #[cfg(target_os = "windows")]
            "nsis" => Ok(PackageFormat::Nsis),
            _ => bail!("unsupported format: {s}"),
        }
    }
}

/// Resolves the signing policy for a packaging operation.
fn resolve_sign_config(
    sign_requested: bool,
    config: &PackagingConfig,
) -> Result<Option<SignConfig>> {
    if !sign_requested {
        return Ok(None);
    }

    SignConfig::from_file_config(&config.signing)
        .map(Some)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "--sign requested but no usable [signing] configuration found in {} \
                 (set `certificate` or `custom-command`)",
                kurogane_layout::CONFIG_FILE_NAME
            )
        })
}

/// Build the application in the requested profile.
pub fn run(debug: bool, format: PackageFormat, sign: bool) -> Result<()> {
    tui::section("Kurogane Bundle");

    // Declarative packaging configuration; defaults when absent
    let metadata = MetadataCommand::new().exec()?;

    let packaging_config = PackagingConfig::load(metadata.workspace_root.as_std_path())?;

    // Build frontend before cargo build
    build_frontend(metadata.workspace_root.as_std_path(), &packaging_config.app)?;

    tui::step("Building release...");

    let mut cmd = Command::new("cargo");

    cmd.arg("build");

    if debug {
        cmd.arg("--features").arg("kurogane/debug");
    } else {
        cmd.arg("--release");
    }

    let status = cmd.status()?;

    if !status.success() {
        bail!("Release build failed");
    }

    // Resolve distribution contents
    tui::step("Resolving distribution...");

    let pkg = metadata
        .root_package()
        .ok_or_else(|| anyhow::anyhow!("No root package"))?;

    let profile = if debug { "debug" } else { "release" };
    let target_dir = metadata.target_directory.join(profile);

    // Find binary target
    let target = pkg
        .targets
        .iter()
        .find(|t| t.kind.contains(&TargetKind::Bin))
        .ok_or_else(|| anyhow::anyhow!("No binary target found"))?;

    let exe_name = if cfg!(target_os = "windows") {
        format!("{}.exe", target.name)
    } else {
        target.name.clone()
    };

    let exe_path = target_dir.join(&exe_name);

    if !exe_path.exists() {
        bail!("Executable not found: {:?}", exe_path);
    }

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

    // Materialize the runnable runtime
    let runtime_version = cef
        .provenance
        .as_ref()
        .map(|p| p.cef_version.clone())
        .unwrap_or_else(|| env!("KUROGANE_CEF_VERSION").to_string());

    let runtime_dir = metadata
        .target_directory
        .join("kurogane")
        .join("cef-runtime")
        .join(&runtime_version);

    let cef_runtime = materialize_cef_runtime(&cef.root, runtime_dir.as_std_path())?;

    let frontend = match &packaging_config.app.frontend {
        Some(path) => {
            if path.exists() {
                Some(path.clone())
            } else {
                tui::warn(&format!(
                    "Configured frontend directory '{}' does not exist",
                    path.display()
                ));
                None
            }
        }
        None => {
            tui::info("No frontend directory configured in kurogane.toml");
            None
        }
    };

    let extra_resources = packaging_config
        .bundle
        .resources
        .iter()
        .map(|r| r.to_resolved())
        .collect::<std::result::Result<Vec<_>, _>>()?;

    let dist = ResolvedDistribution {
        metadata: AppMetadata {
            name: packaging_config
                .app
                .name
                .clone()
                .unwrap_or_else(|| pkg.name.to_string()),
            version: pkg.version.to_string(),
            exe_name,
            publisher: packaging_config.app.publisher.clone(),
            description: packaging_config.app.description.clone(),
            copyright: packaging_config.app.copyright.clone(),
            icon: packaging_config.app.icon.clone(),
        },
        executable: exe_path.into(),
        frontend,
        cef_runtime,
        extra_resources,
    };

    dist.validate()
        .map_err(|e| anyhow::anyhow!("distribution validation failed: {e}"))?;

    tui::field("binary", tui::format_path(&dist.executable));
    tui::field("format", format!("{format:?}"));

    // Package the distribution
    tui::step("Packaging...");

    let output_dir = PathBuf::from("dist");
    let sign_config = resolve_sign_config(sign, &packaging_config)?;

    match format {
        PackageFormat::Directory => {
            // The canonical bundle is the artifact; sign it in place
            let output = package_directory(&dist, &output_dir)?;

            if let Some(config) = &sign_config {
                let signed = sign_tree(&output, config)?;
                tui::field("signed", format!("{signed} file(s)"));
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
    }

    tui::blank();
    tui::success("Bundle ready");
    tui::field("path", "./dist");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
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
    fn rejects_unsupported_format() {
        let err = PackageFormat::from_str("msi").unwrap_err();

        assert!(err.to_string().contains("unsupported format"));
    }

    #[test]
    fn rejects_empty_format() {
        assert!(PackageFormat::from_str("").is_err());
    }
}

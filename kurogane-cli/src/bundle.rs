use anyhow::{Result, bail};
use std::path::PathBuf;
use std::process::Command;
use cargo_metadata::{MetadataCommand, TargetKind};
use kurogane_layout::{
    AppMetadata, ResolvedDistribution, materialize_cef_runtime, package_directory,
    resolve_cef_for_bundle,
};

#[allow(unused_imports)]
use crate::signing::SignConfig;
use crate::tui;

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

/// Build the application in the requested profile.
#[allow(unused_variables)]
pub fn run(debug: bool, format: PackageFormat, sign_config: Option<SignConfig>) -> Result<()> {
    tui::section("Kurogane Bundle");

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

    let metadata = MetadataCommand::new().exec()?;

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

    let cef = resolve_cef_for_bundle(env!("KUROGANE_CEF_VERSION"))
        .map_err(|e| anyhow::anyhow!(e))?;

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

    let cef_runtime =
        materialize_cef_runtime(&cef.root, runtime_dir.as_std_path()).map_err(|e| anyhow::anyhow!(e))?;

    let frontend = {
        let path = PathBuf::from("content");
        if path.exists() {
            Some(path)
        } else {
            tui::warn("No content/ directory found");
            None
        }
    };

    let dist = ResolvedDistribution {
        metadata: AppMetadata {
            name: pkg.name.to_string(),
            version: pkg.version.to_string(),
            exe_name,
        },
        executable: exe_path.into(),
        frontend,
        cef_runtime,
        extra_resources: Vec::new(),
    };

    dist.validate()
        .map_err(|e| anyhow::anyhow!("distribution validation failed: {e}"))?;

    tui::field("binary", tui::format_path(&dist.executable));
    tui::field("format", format!("{format:?}"));

    // Package the distribution
    tui::step("Packaging...");

    let output_dir = PathBuf::from("dist");

    match format {
        PackageFormat::Directory => {
            let output = package_directory(&dist, &output_dir)?;
            tui::field("output", tui::format_path(&output));
        }

        #[cfg(target_os = "linux")]
        PackageFormat::AppImage => {
            crate::appimage::build(&dist, &output_dir)?;
        }

        #[cfg(target_os = "windows")]
        PackageFormat::Nsis => {
            crate::nsis::build(&dist, &output_dir)?;
        }
    }

    println!();
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

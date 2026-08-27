//! Represents an application bundle as a materialized directory.

use std::path::{Path, PathBuf};
use thiserror::Error;

use crate::{BundleLayout, ResolvedDistribution};

#[derive(Debug, Error)]
pub enum PackageError {
    #[error(transparent)]
    Layout(#[from] crate::BundleError),

    #[error(transparent)]
    Distribution(#[from] crate::DistributionError),
}

/// Packages a resolved distribution as a plain directory bundle.
///
/// This is the baseline materializer. Other formats (AppImage, NSIS) build
/// on the same `ResolvedDistribution` input but produce different artifacts.
pub fn package_directory(
    dist: &ResolvedDistribution,
    output_dir: &Path,
) -> Result<PathBuf, PackageError> {
    let layout = BundleLayout::new(output_dir);
    layout.materialize(dist)?;

    let exe_name = dist
        .executable
        .file_name()
        .ok_or_else(|| crate::BundleError::InvalidExecutablePath(dist.executable.clone()))?;
    layout.verify(exe_name)?;

    Ok(layout.root().to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppMetadata;
    use std::fs;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().expect("failed to create temp dir")
    }

    fn create_cef_fixture(dir: &Path) -> PathBuf {
        crate::cef::write_runtime_fixture(&dir.join("cef"))
    }

    fn valid_distribution(dir: &Path) -> ResolvedDistribution {
        #[cfg(target_os = "windows")]
        let exe_name = "myapp.exe";
        #[cfg(not(target_os = "windows"))]
        let exe_name = "myapp";

        let exe = dir.join(exe_name);
        fs::write(&exe, "binary").unwrap();

        let frontend = dir.join("frontend");
        fs::create_dir_all(&frontend).unwrap();
        fs::write(frontend.join("index.html"), "<html></html>").unwrap();

        let cef = create_cef_fixture(dir);

        ResolvedDistribution {
            metadata: AppMetadata {
                name: "myapp".to_string(),
                version: "1.0.0".to_string(),
                exe_name: "myapp".to_string(),
                ..Default::default()
            },
            executable: exe,
            frontend: Some(frontend),
            cef_runtime: cef,
            extra_resources: Vec::new(),
        }
    }

    #[test]
    fn package_directory_returns_materialized_bundle() {
        let dir = tmp();
        let dist = valid_distribution(dir.path());
        let out = dir.path().join("dist");

        let result = package_directory(&dist, &out).unwrap();
        assert_eq!(result, out);
        assert!(result.is_dir(), "output directory should exist");
    }

    #[test]
    fn package_directory_contains_executable() {
        let dir = tmp();
        let dist = valid_distribution(dir.path());
        let out = dir.path().join("dist");

        let bundle = package_directory(&dist, &out).unwrap();

        #[cfg(target_os = "linux")]
        let exe = bundle.join("runtime").join("myapp");
        #[cfg(target_os = "windows")]
        let exe = bundle.join("myapp.exe");

        assert!(exe.exists(), "bundled executable should exist");
    }

    #[test]
    fn package_directory_contains_cef() {
        let dir = tmp();
        let dist = valid_distribution(dir.path());
        let out = dir.path().join("dist");

        let bundle = package_directory(&dist, &out).unwrap();

        #[cfg(target_os = "linux")]
        let libcef = bundle.join("runtime").join("cef").join("libcef.so");
        #[cfg(target_os = "windows")]
        let libcef = bundle.join("libcef.dll");

        assert!(libcef.exists(), "libcef should be in the bundle");
    }

    #[test]
    fn package_directory_contains_frontend() {
        let dir = tmp();
        let dist = valid_distribution(dir.path());
        let out = dir.path().join("dist");

        let bundle = package_directory(&dist, &out).unwrap();
        let index = bundle.join("content").join("index.html");
        assert!(index.exists(), "index.html should be in the bundle");
    }

    #[test]
    fn package_directory_without_frontend() {
        let dir = tmp();
        let mut dist = valid_distribution(dir.path());
        dist.frontend = None;

        let out = dir.path().join("dist");
        let bundle = package_directory(&dist, &out).unwrap();

        // Verify the bundle was created successfully
        assert!(bundle.exists());

        // content/ should not exist
        assert!(
            !bundle.join("content").exists(),
            "content directory should not be created"
        );
    }

    #[test]
    fn package_directory_contains_extra_resources() {
        let dir = tmp();
        let mut dist = valid_distribution(dir.path());

        let res = dir.path().join("extra.txt");
        fs::write(&res, "data").unwrap();
        dist.extra_resources.push(crate::ResolvedResource {
            source: res.clone(),
            destination: "extra.txt".into(),
        });

        let out = dir.path().join("dist");
        let bundle = package_directory(&dist, &out).unwrap();

        assert!(
            bundle.join("extra.txt").exists(),
            "extra resource should be in bundle"
        );
    }

    #[test]
    fn package_directory_rejects_invalid_distribution() {
        let dir = tmp();
        let mut dist = valid_distribution(dir.path());
        dist.executable = dir.path().join("nonexistent");

        let out = dir.path().join("dist");
        let result = package_directory(&dist, &out);
        assert!(result.is_err(), "should fail with missing executable");
    }
}

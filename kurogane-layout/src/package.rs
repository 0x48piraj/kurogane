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

    #[test]
    fn package_directory_returns_materialized_bundle() {
        let dir = crate::test_fixtures::tmp_dir();
        let dist = crate::test_fixtures::sample_distribution(dir.path());
        let out = dir.path().join("dist");

        let result = package_directory(&dist, &out).unwrap();
        assert_eq!(result, out);
        assert!(result.is_dir(), "output directory should exist");
    }

    #[test]
    fn package_directory_contains_executable() {
        let dir = crate::test_fixtures::tmp_dir();
        let dist = crate::test_fixtures::sample_distribution(dir.path());
        let out = dir.path().join("dist");

        let bundle = package_directory(&dist, &out).unwrap();

        #[cfg(target_os = "linux")]
        let exe = bundle.join("runtime").join("myapp");
        #[cfg(target_os = "windows")]
        let exe = bundle.join("myapp.exe");
        #[cfg(target_os = "macos")]
        let exe = bundle.join("myapp");

        assert!(exe.exists(), "bundled executable should exist");
    }

    #[test]
    fn package_directory_contains_cef() {
        let dir = crate::test_fixtures::tmp_dir();
        let dist = crate::test_fixtures::sample_distribution(dir.path());
        let out = dir.path().join("dist");

        let bundle = package_directory(&dist, &out).unwrap();

        #[cfg(target_os = "linux")]
        let libcef = bundle.join("runtime").join("cef").join("libcef.so");
        #[cfg(target_os = "windows")]
        let libcef = bundle.join("libcef.dll");
        #[cfg(target_os = "macos")]
        let libcef = bundle
            .join("Chromium Embedded Framework.framework")
            .join("Chromium Embedded Framework");

        assert!(libcef.exists(), "CEF binary should be in the bundle");
    }

    #[test]
    fn package_directory_contains_frontend() {
        let dir = crate::test_fixtures::tmp_dir();
        let dist = crate::test_fixtures::sample_distribution(dir.path());
        let out = dir.path().join("dist");

        let bundle = package_directory(&dist, &out).unwrap();
        let index = bundle.join("content").join("index.html");
        assert!(index.exists(), "index.html should be in the bundle");
    }

    #[test]
    fn package_directory_without_frontend() {
        let dir = crate::test_fixtures::tmp_dir();
        let mut dist = crate::test_fixtures::sample_distribution(dir.path());
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
        let dir = crate::test_fixtures::tmp_dir();
        let dist = crate::test_fixtures::sample_distribution(dir.path());

        let out = dir.path().join("dist");
        let bundle = package_directory(&dist, &out).unwrap();

        assert!(
            bundle.join("extra.txt").exists(),
            "extra resource should be in bundle"
        );
    }

    #[test]
    fn package_directory_rejects_invalid_distribution() {
        let dir = crate::test_fixtures::tmp_dir();
        let mut dist = crate::test_fixtures::sample_distribution(dir.path());
        dist.executable = dir.path().join("nonexistent");

        let out = dir.path().join("dist");
        let result = package_directory(&dist, &out);
        assert!(result.is_err(), "should fail with missing executable");
    }
}

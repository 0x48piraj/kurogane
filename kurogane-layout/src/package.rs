use std::path::{Path, PathBuf};
use thiserror::Error;

use crate::{BundleLayout, ResolvedDistribution};

#[derive(Debug, Error)]
pub enum PackageError {
    #[error(transparent)]
    Layout(#[from] anyhow::Error),

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
        .ok_or_else(|| anyhow::anyhow!("executable has no file name"))?;
    layout.verify(exe_name)?;

    Ok(layout.root().to_path_buf())
}

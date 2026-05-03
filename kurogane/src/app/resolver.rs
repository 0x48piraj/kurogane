//! Internal frontend resolution logic.

use std::path::{Path, PathBuf};

use super::Source;

use crate::error::RuntimeError;

/// Result of frontend resolution.
pub struct ResolvedFrontend {
    pub asset_root: Option<PathBuf>,
    pub start_url: String,
}

const APP_URL: &str = "app://app/index.html";

/// Resolve the frontend entrypoint.
///
/// Priority:
/// 1. Explicit URL  (App::url)
/// 2. Explicit path (App::new)
///
/// Errors if no valid frontend is found.
pub(crate) fn resolve(source: &Source) -> Result<ResolvedFrontend, RuntimeError> {
    match source {
        Source::Url(url) => {
            Ok(ResolvedFrontend {
                asset_root: None,
                start_url: url.clone(),
            })
        }

        Source::Path(dir) => {
            let dir = normalize_path(dir)?;

            validate_asset_root(&dir)?;

            Ok(ResolvedFrontend {
                asset_root: Some(dir),
                start_url: APP_URL.to_string(),
            })
        }
    }
}

/// Convert path to absolute, stable form.
fn normalize_path(path: &Path) -> Result<PathBuf, RuntimeError> {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|_| RuntimeError::AssetRootMissing(path.to_path_buf()))?
            .join(path)
    };

    // Canonicalize
    abs.canonicalize()
        .map_err(|_| RuntimeError::AssetRootMissing(abs))
}

/// Ensure directory exists and contains index.html.
fn validate_asset_root(dir: &Path) -> Result<(), RuntimeError> {
    if !dir.is_dir() {
        return Err(RuntimeError::InvalidAssetRoot(dir.to_path_buf()));
    }

    let index = dir.join("index.html");

    if !index.is_file() {
        return Err(RuntimeError::AssetRootMissing(dir.to_path_buf()));
    }

    Ok(())
}

use std::path::PathBuf;
use thiserror::Error;

use crate::cef::{CefProvenance, read_provenance};
use crate::{bundled_cef_root, installed_cef_root};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryMode {
    EnvironmentOverride,
    Bundled,
    Installed,
}

#[derive(Debug)]
pub struct DetectedCef {
    pub root: PathBuf,
    pub mode: DiscoveryMode,
    /// Provenance when available.
    pub provenance: Option<CefProvenance>,
}

#[derive(Debug, Error)]
pub enum DetectError {
    #[error("CEF runtime not found")]
    NotFound,

    #[error("failed to determine executable path")]
    CurrentExe(#[from] std::io::Error),
}

/// Resolves the active CEF runtime using discovery precedence rules.
pub fn detect_cef_root_with_version(version: Option<&str>) -> Result<DetectedCef, DetectError> {
    // Environment override
    if let Ok(path) = std::env::var("CEF_PATH") {
        let root = PathBuf::from(path);

        if root.exists() {
            return Ok(DetectedCef {
                root,
                mode: DiscoveryMode::EnvironmentOverride,
                provenance: None,
            });
        }
    }

    // Bundled runtime (next to executable)
    if let Some(root) = bundled_cef_root()? {
        return Ok(DetectedCef {
            root,
            mode: DiscoveryMode::Bundled,
            provenance: None,
        });
    }

    // Managed installation
    if let Some(root) = version.and_then(installed_cef_root) {
        return Ok(DetectedCef {
            provenance: read_provenance(&root).ok().flatten(),
            root,
            mode: DiscoveryMode::Installed,
        });
    }

    Err(DetectError::NotFound)
}

use std::path::PathBuf;
use thiserror::Error;

use crate::bundled_cef_root;

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
}

#[derive(Debug, Error)]
pub enum DetectError {
    #[error("CEF runtime not found")]
    NotFound,

    #[error("failed to determine executable path")]
    CurrentExe(#[from] std::io::Error),
}

/// Resolves the active CEF runtime using discovery precedence rules.
pub fn detect_cef_root() -> Result<DetectedCef, DetectError> {
    // Dev environment
    if let Ok(path) = std::env::var("CEF_PATH") {
        let root = PathBuf::from(path);

        if root.exists() {
            return Ok(DetectedCef {
                root,
                mode: DiscoveryMode::EnvironmentOverride,
            });
        }
    }

    // Bundled runtime (next to executable)
    if let Some(root) = bundled_cef_root()? {
        return Ok(DetectedCef {
            root,
            mode: DiscoveryMode::Bundled,
        });
    }

    Err(DetectError::NotFound)
}

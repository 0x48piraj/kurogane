//! Runtime CEF discovery.
//!
//! This module resolves the CEF runtime used by a running application,
//! following the configured precedence between environment overrides,
//! bundled runtimes and managed installations.

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

impl std::fmt::Display for DiscoveryMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EnvironmentOverride => write!(f, "Environment override"),
            Self::Bundled => write!(f, "Bundled"),
            Self::Installed => write!(f, "Installed"),
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use std::sync::Mutex;

    fn tmp() -> tempfile::TempDir {
        crate::test_fixtures::tmp_dir()
    }

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn with_cef_path<T>(path: Option<&Path>, f: impl FnOnce() -> T) -> T {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let original = std::env::var_os("CEF_PATH");

        // SAFETY: CEF_PATH access is serialized by ENV_LOCK
        unsafe {
            match path {
                Some(path) => std::env::set_var("CEF_PATH", path),
                None => std::env::remove_var("CEF_PATH"),
            }
        }

        let result = f();

        // SAFETY: CEF_PATH access is serialized by ENV_LOCK
        unsafe {
            match original {
                Some(value) => std::env::set_var("CEF_PATH", value),
                None => std::env::remove_var("CEF_PATH"),
            }
        }

        result
    }

    #[test]
    fn env_override_takes_precedence() {
        let dir = tmp();
        let cef = dir.path().join("cef");
        fs::create_dir(&cef).unwrap();

        let detected = with_cef_path(Some(&cef), || detect_cef_root_with_version(None).unwrap());

        assert_eq!(detected.mode, DiscoveryMode::EnvironmentOverride);
        assert_eq!(detected.root, cef);
    }

    #[test]
    fn env_override_with_invalid_path_is_skipped() {
        let dir = tmp();
        let nonexistent = dir.path().join("nonexistent");

        let result = with_cef_path(Some(&nonexistent), || detect_cef_root_with_version(None));

        assert!(result.is_err());
    }

    #[test]
    fn version_none_skips_installed_check() {
        let result = with_cef_path(None, || detect_cef_root_with_version(None));

        assert!(result.is_err());
    }

    #[test]
    fn nonexistent_version_returns_not_found() {
        let result = with_cef_path(None, || {
            detect_cef_root_with_version(Some("0.0.0-nonexistent-version"))
        });

        assert!(matches!(result, Err(DetectError::NotFound)));
    }
}

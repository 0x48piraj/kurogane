//! Runtime CEF discovery.
//!
//! Each command has its own rule for where CEF is, and they share one
//! reader of `CEF_PATH`, [`cef_override`]:
//!
//! - a running application ([`detect_cef_root`]): a bundle's own runtime and
//!   no other; outside a bundle, a runtime beside the executable, else
//!   `CEF_PATH`;
//! - `kurogane dev`, `run` and `build`: `CEF_PATH`, else the managed
//!   installation, which they then pass on as `CEF_PATH`;
//! - `kurogane bundle` ([`crate::resolve_cef_for_bundle`]): `CEF_PATH`, else
//!   the managed installation, each with verified provenance, copied into
//!   the bundle.

use std::path::PathBuf;
use thiserror::Error;

use crate::bundled_cef_root;
use crate::layout::bundle_cef_root_for;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryMode {
    /// The runtime inside the bundle the executable belongs to.
    Bundled,
    /// A runtime beside an executable that belongs to no bundle, such as the
    /// copy cef-dll-sys leaves in Cargo's target directory on Windows.
    BesideExecutable,
    EnvironmentOverride,
}

impl std::fmt::Display for DiscoveryMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bundled => write!(f, "Bundled"),
            Self::BesideExecutable => write!(f, "Beside the executable"),
            Self::EnvironmentOverride => write!(f, "Environment override"),
        }
    }
}

#[derive(Debug)]
pub struct DetectedCef {
    pub root: PathBuf,
    pub mode: DiscoveryMode,
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum DetectError {
    #[error("CEF runtime not found")]
    NotFound,

    #[error("failed to determine executable path")]
    CurrentExe(#[from] std::io::Error),
}

/// The CEF distribution `CEF_PATH` names, when it is set and not empty.
pub fn cef_override() -> Option<PathBuf> {
    std::env::var_os("CEF_PATH")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
}

/// Resolves the CEF runtime of the running application.
///
/// A bundle `kurogane bundle` made runs the runtime inside it and no other,
/// whatever `CEF_PATH` a user has set: it loads the libcef it shipped with,
/// so resources and locales must come from that same tree. When that runtime
/// is gone the bundle is reported incomplete rather than run on another.
///
/// Any other executable uses a runtime beside it, else the one `CEF_PATH`
/// names, as `kurogane dev` and `run` set it.
pub fn detect_cef_root() -> Result<DetectedCef, DetectError> {
    let exe = std::env::current_exe()?;
    detect(
        bundle_cef_root_for(&exe),
        bundled_cef_root()?,
        cef_override(),
    )
}

fn detect(
    bundle: Option<PathBuf>,
    beside: Option<PathBuf>,
    overridden: Option<PathBuf>,
) -> Result<DetectedCef, DetectError> {
    // Not checked for presence: the caller's validation says what is missing
    if let Some(root) = bundle {
        return Ok(DetectedCef {
            root,
            mode: DiscoveryMode::Bundled,
        });
    }

    if let Some(root) = beside {
        return Ok(DetectedCef {
            root,
            mode: DiscoveryMode::BesideExecutable,
        });
    }

    if let Some(root) = overridden.filter(|root| root.exists()) {
        return Ok(DetectedCef {
            root,
            mode: DiscoveryMode::EnvironmentOverride,
        });
    }

    Err(DetectError::NotFound)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp() -> tempfile::TempDir {
        crate::test_fixtures::tmp_dir()
    }

    #[test]
    fn a_bundle_runs_its_own_runtime_whatever_the_override() {
        let dir = tmp();
        let bundled = dir.path().join("bundled");
        let overridden = dir.path().join("override");
        fs::create_dir(&bundled).unwrap();
        fs::create_dir(&overridden).unwrap();

        let detected = detect(Some(bundled.clone()), None, Some(overridden)).unwrap();

        assert_eq!(detected.mode, DiscoveryMode::Bundled);
        assert_eq!(detected.root, bundled);
    }

    #[test]
    fn a_bundle_whose_runtime_is_gone_falls_back_to_nothing() {
        let dir = tmp();
        let gone = dir.path().join("gone");
        let beside = dir.path().join("beside");
        let overridden = dir.path().join("override");
        fs::create_dir(&beside).unwrap();
        fs::create_dir(&overridden).unwrap();

        let detected = detect(Some(gone.clone()), Some(beside), Some(overridden)).unwrap();

        assert_eq!(detected.mode, DiscoveryMode::Bundled);
        assert_eq!(detected.root, gone);
    }

    #[test]
    fn a_runtime_beside_the_executable_takes_precedence_over_the_override() {
        let dir = tmp();
        let beside = dir.path().join("beside");
        let overridden = dir.path().join("override");
        fs::create_dir(&beside).unwrap();
        fs::create_dir(&overridden).unwrap();

        let detected = detect(None, Some(beside.clone()), Some(overridden)).unwrap();

        assert_eq!(detected.mode, DiscoveryMode::BesideExecutable);
        assert_eq!(detected.root, beside);
    }

    #[test]
    fn the_override_serves_an_application_without_a_bundle() {
        let dir = tmp();
        let cef = dir.path().join("cef");
        fs::create_dir(&cef).unwrap();

        let detected = detect(None, None, Some(cef.clone())).unwrap();

        assert_eq!(detected.mode, DiscoveryMode::EnvironmentOverride);
        assert_eq!(detected.root, cef);
    }

    #[test]
    fn a_missing_override_is_skipped() {
        let dir = tmp();
        let nonexistent = dir.path().join("nonexistent");

        let result = detect(None, None, Some(nonexistent));

        assert!(matches!(result, Err(DetectError::NotFound)));
    }

    #[test]
    fn nothing_bundled_and_no_override_is_not_found() {
        assert!(matches!(
            detect(None, None, None),
            Err(DetectError::NotFound)
        ));
    }
}

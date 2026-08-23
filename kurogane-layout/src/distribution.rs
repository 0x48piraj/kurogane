//! Resolved application distribution model.
//!
//! This module defines the platform-independent description of the files
//! required to distribute an application and validates that all declared
//! inputs are usable.

use std::path::{Path, PathBuf};
use thiserror::Error;

/// Application identity and distribution metadata.
#[derive(Debug, Clone, Default)]
pub struct AppMetadata {
    pub name: String,
    pub version: String,
    pub exe_name: String,
    pub publisher: Option<String>,
    pub description: Option<String>,
    pub copyright: Option<String>,
    pub icon: Option<PathBuf>,
}

/// Resolved resource source and bundle destination.
#[derive(Debug, Clone)]
pub struct ResolvedResource {
    pub source: PathBuf,
    pub destination: PathBuf,
}

/// The resolved contents of an application distribution.
///
/// Describes what must be distributed without prescribing how it is
/// packaged or laid out on disk.
///
/// Platform-specific layout is the responsibility of the materializer.
#[derive(Debug, Clone)]
pub struct ResolvedDistribution {
    pub metadata: AppMetadata,
    pub executable: PathBuf,
    pub frontend: Option<PathBuf>,
    /// Materialized CEF runtime.
    pub cef_runtime: PathBuf,
    pub extra_resources: Vec<ResolvedResource>,
}

#[derive(Debug, Error)]
pub enum DistributionError {
    #[error("executable not found: {0}")]
    MissingExecutable(PathBuf),

    #[error("executable path is not a file: {0}")]
    ExecutableNotFile(PathBuf),

    #[error("frontend directory not found: {0}")]
    MissingFrontend(PathBuf),

    #[error("frontend path is not a directory: {0}")]
    FrontendNotDir(PathBuf),

    #[error("frontend missing index.html at {0}")]
    MissingIndex(PathBuf),

    #[error("CEF runtime not found: {0}")]
    MissingCefRoot(PathBuf),

    #[error("CEF runtime is not a directory: {0}")]
    CefRootNotDir(PathBuf),

    #[error("extra resource not found: {0}")]
    MissingResource(PathBuf),

    #[error("resource destination must be a relative path without '..' components: {0}")]
    InvalidResourceDestination(PathBuf),

    #[error("invalid CEF runtime: {0}")]
    InvalidCefRuntime(#[from] crate::cef::CefError),
}

impl ResolvedDistribution {
    /// Validates the resolved distribution.
    pub fn validate(&self) -> Result<(), DistributionError> {
        if !self.executable.exists() {
            return Err(DistributionError::MissingExecutable(
                self.executable.clone(),
            ));
        }

        if !self.executable.is_file() {
            return Err(DistributionError::ExecutableNotFile(
                self.executable.clone(),
            ));
        }

        if let Some(frontend) = &self.frontend {
            if !frontend.exists() {
                return Err(DistributionError::MissingFrontend(frontend.clone()));
            }

            if !frontend.is_dir() {
                return Err(DistributionError::FrontendNotDir(frontend.clone()));
            }

            let index = frontend.join("index.html");
            if !index.exists() {
                return Err(DistributionError::MissingIndex(index));
            }
        }

        if !self.cef_runtime.exists() {
            return Err(DistributionError::MissingCefRoot(self.cef_runtime.clone()));
        }

        if !self.cef_runtime.is_dir() {
            return Err(DistributionError::CefRootNotDir(self.cef_runtime.clone()));
        }

        self.validate_cef()?;

        for resource in &self.extra_resources {
            if !resource.source.exists() {
                return Err(DistributionError::MissingResource(resource.source.clone()));
            }

            validate_resource_destination(&resource.destination)?;
        }

        Ok(())
    }

    fn validate_cef(&self) -> Result<(), DistributionError> {
        crate::cef::validate_cef_runtime(&self.cef_runtime)?;
        Ok(())
    }
}

/// Validates a resource destination within the bundle.
///
/// Fail-fast guard against authoring mistakes, not a security boundary.
fn validate_resource_destination(destination: &Path) -> Result<(), DistributionError> {
    let escapes = destination.has_root()
        || destination
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir));

    if escapes {
        return Err(DistributionError::InvalidResourceDestination(
            destination.to_path_buf(),
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().expect("failed to create temp dir")
    }

    fn valid_distribution(dir: &Path) -> ResolvedDistribution {
        #[cfg(target_os = "windows")]
        let exe_name = "myapp.exe";
        #[cfg(not(target_os = "windows"))]
        let exe_name = "myapp";

        let exe = dir.join(exe_name);
        fs::write(&exe, "binary").unwrap();

        let frontend = dir.join("content");
        fs::create_dir_all(&frontend).unwrap();
        fs::write(frontend.join("index.html"), "<html></html>").unwrap();

        let cef = crate::cef::write_runtime_fixture(&dir.join("cef"));

        let resource = dir.join("extra.txt");
        fs::write(&resource, "data").unwrap();

        ResolvedDistribution {
            metadata: AppMetadata {
                name: "myapp".to_string(),
                version: "1.0.0".to_string(),
                exe_name: exe_name.to_string(),
                ..Default::default()
            },
            executable: exe,
            frontend: Some(frontend),
            cef_runtime: cef,
            extra_resources: vec![ResolvedResource {
                source: resource.clone(),
                destination: resource
                    .file_name()
                    .map(Into::into)
                    .unwrap_or_else(|| "extra.txt".into()),
            }],
        }
    }

    #[test]
    fn valid_distribution_passes_validation() {
        let dir = tmp();
        let dist = valid_distribution(dir.path());
        assert!(dist.validate().is_ok());
    }

    #[test]
    fn missing_executable_is_rejected() {
        let dir = tmp();
        let dist = valid_distribution(dir.path());
        fs::remove_file(&dist.executable).unwrap();

        let err = dist.validate().unwrap_err();
        assert!(
            matches!(err, DistributionError::MissingExecutable(ref p) if p == &dist.executable),
            "expected MissingExecutable, got: {err}"
        );
    }

    #[test]
    fn executable_is_directory_rejected() {
        let dir = tmp();
        let dist = valid_distribution(dir.path());
        fs::remove_file(&dist.executable).unwrap();
        fs::create_dir(&dist.executable).unwrap();

        let err = dist.validate().unwrap_err();
        assert!(
            matches!(err, DistributionError::ExecutableNotFile(_)),
            "expected ExecutableNotFile, got: {err}"
        );
    }

    #[test]
    fn missing_frontend_directory_is_rejected() {
        let dir = tmp();
        let mut dist = valid_distribution(dir.path());
        dist.frontend = Some(dir.path().join("nonexistent"));

        let err = dist.validate().unwrap_err();
        assert!(
            matches!(err, DistributionError::MissingFrontend(_)),
            "expected MissingFrontend, got: {err}"
        );
    }

    #[test]
    fn frontend_not_a_directory_is_rejected() {
        let dir = tmp();
        let mut dist = valid_distribution(dir.path());
        let file_path = dir.path().join("not_a_dir");
        fs::write(&file_path, "content").unwrap();
        dist.frontend = Some(file_path);

        let err = dist.validate().unwrap_err();
        assert!(
            matches!(err, DistributionError::FrontendNotDir(_)),
            "expected FrontendNotDir, got: {err}"
        );
    }

    #[test]
    fn missing_index_html_is_rejected() {
        let dir = tmp();
        let mut dist = valid_distribution(dir.path());
        let empty_frontend = dir.path().join("empty_frontend");
        fs::create_dir(&empty_frontend).unwrap();
        dist.frontend = Some(empty_frontend);

        let err = dist.validate().unwrap_err();
        assert!(
            matches!(err, DistributionError::MissingIndex(_)),
            "expected MissingIndex, got: {err}"
        );
    }

    #[test]
    fn frontend_none_does_not_require_index() {
        let dir = tmp();
        let mut dist = valid_distribution(dir.path());
        dist.frontend = None;

        assert!(
            dist.validate().is_ok(),
            "frontend=None should pass validation"
        );
    }

    #[test]
    fn missing_cef_root_is_rejected() {
        let dir = tmp();
        let mut dist = valid_distribution(dir.path());
        dist.cef_runtime = dir.path().join("nonexistent_cef");

        let err = dist.validate().unwrap_err();
        assert!(
            matches!(err, DistributionError::MissingCefRoot(_)),
            "expected MissingCefRoot, got: {err}"
        );
    }

    #[test]
    fn cef_root_not_a_directory_is_rejected() {
        let dir = tmp();
        let mut dist = valid_distribution(dir.path());
        let file_path = dir.path().join("not_a_cef_dir");
        fs::write(&file_path, "").unwrap();
        dist.cef_runtime = file_path;

        let err = dist.validate().unwrap_err();
        assert!(
            matches!(err, DistributionError::CefRootNotDir(_)),
            "expected CefRootNotDir, got: {err}"
        );
    }

    #[test]
    fn incomplete_cef_runtime_is_rejected() {
        let dir = tmp();
        let mut dist = valid_distribution(dir.path());
        let empty_cef = dir.path().join("empty_cef");
        fs::create_dir(&empty_cef).unwrap();
        dist.cef_runtime = empty_cef;

        let err = dist.validate().unwrap_err();
        assert!(
            matches!(err, DistributionError::InvalidCefRuntime(_)),
            "expected InvalidCefRuntime, got: {err}"
        );
    }

    #[test]
    fn missing_extra_resource_is_rejected() {
        let dir = tmp();
        let mut dist = valid_distribution(dir.path());
        let missing = dir.path().join("nonexistent_resource");
        dist.extra_resources = vec![ResolvedResource {
            source: missing.clone(),
            destination: "nonexistent_resource".into(),
        }];

        let err = dist.validate().unwrap_err();
        assert!(
            matches!(err, DistributionError::MissingResource(ref p) if p == &missing),
            "expected MissingResource, got: {err}"
        );
    }

    #[test]
    fn missing_resource_not_confused_with_cef_or_frontend_error() {
        let dir = tmp();
        let mut dist = valid_distribution(dir.path());
        dist.extra_resources = vec![ResolvedResource {
            source: dir.path().join("missing_res"),
            destination: "missing_res".into(),
        }];

        let err = dist.validate().unwrap_err();
        assert!(!matches!(
            err,
            DistributionError::MissingCefRoot(_)
                | DistributionError::InvalidCefRuntime(_)
                | DistributionError::MissingFrontend(_)
                | DistributionError::MissingIndex(_)
        ));
    }

    #[test]
    fn raw_distribution_root_is_not_a_valid_runtime() {
        let dir = tmp();
        let raw = dir.path().join("raw_dist");
        fs::create_dir_all(raw.join("Release")).unwrap();
        fs::create_dir_all(raw.join("Resources")).unwrap();

        let dist = ResolvedDistribution {
            metadata: AppMetadata {
                name: "test".to_string(),
                version: "0.1.0".to_string(),
                exe_name: "test".to_string(),
                ..Default::default()
            },
            executable: {
                let e = dir.path().join("test");
                fs::write(&e, "").unwrap();
                e
            },
            frontend: None,
            cef_runtime: raw,
            extra_resources: Vec::new(),
        };

        assert!(
            dist.validate().is_err(),
            "raw distribution root must fail runtime validation"
        );
    }

    #[test]
    fn extra_resources_dirs_are_checked() {
        let dir = tmp();
        let mut dist = valid_distribution(dir.path());
        let missing_dir = dir.path().join("missing_dir");
        dist.extra_resources = vec![ResolvedResource {
            source: missing_dir,
            destination: "missing_dir".into(),
        }];

        let err = dist.validate().unwrap_err();
        assert!(
            matches!(err, DistributionError::MissingResource(_)),
            "expected MissingResource for missing directory, got: {err}"
        );
    }

    #[test]
    fn absolute_resource_destination_is_rejected() {
        let dir = tmp();
        let mut dist = valid_distribution(dir.path());
        dist.extra_resources = vec![ResolvedResource {
            source: dir.path().join("extra.txt"),
            destination: "/etc/passwd".into(),
        }];

        let err = dist.validate().unwrap_err();
        assert!(
            matches!(err, DistributionError::InvalidResourceDestination(_)),
            "expected InvalidResourceDestination for absolute path, got: {err}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn drive_letter_destination_is_rejected_on_windows() {
        let dir = tmp();
        let mut dist = valid_distribution(dir.path());
        dist.extra_resources = vec![ResolvedResource {
            source: dir.path().join("extra.txt"),
            destination: r"C:\Windows\evil.dll".into(),
        }];

        let err = dist.validate().unwrap_err();
        assert!(matches!(
            err,
            DistributionError::InvalidResourceDestination(_)
        ));
    }

    #[test]
    fn parent_dir_resource_destination_is_rejected() {
        let dir = tmp();
        let mut dist = valid_distribution(dir.path());
        dist.extra_resources = vec![ResolvedResource {
            source: dir.path().join("extra.txt"),
            destination: "../escape.txt".into(),
        }];

        let err = dist.validate().unwrap_err();
        assert!(
            matches!(err, DistributionError::InvalidResourceDestination(ref p) if p == &PathBuf::from("../escape.txt")),
            "expected InvalidResourceDestination for '..' path, got: {err}"
        );
    }

    #[test]
    fn nested_relative_resource_destination_is_accepted() {
        let dir = tmp();
        let mut dist = valid_distribution(dir.path());
        dist.extra_resources = vec![ResolvedResource {
            source: dir.path().join("extra.txt"),
            destination: "share/data/extra.txt".into(),
        }];

        dist.validate().unwrap();
    }

    #[test]
    fn exe_name_matches_executable_filename() {
        let dir = tmp();
        let dist = valid_distribution(dir.path());

        let actual_filename = dist.executable.file_name().unwrap().to_str().unwrap();

        assert_eq!(
            dist.metadata.exe_name, actual_filename,
            "exe_name should match the actual executable filename"
        );
    }
}

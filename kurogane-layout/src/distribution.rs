use std::path::PathBuf;
use thiserror::Error;

/// Application identity and metadata for the distribution.
#[derive(Debug, Clone)]
pub struct AppMetadata {
    pub name: String,
    pub version: String,
    pub exe_name: String,
}

/// The resolved contents of an application distribution.
///
/// Describes what must be distributed without prescribing how it is
/// packaged or laid out on disk.
/// Platform-specific layout is the responsibility of the materializer.
#[derive(Debug, Clone)]
pub struct ResolvedDistribution {
    pub metadata: AppMetadata,
    pub executable: PathBuf,
    pub frontend: Option<PathBuf>,
    /// A runnable, materialized CEF runtime.
    pub cef_runtime: PathBuf,
    pub extra_resources: Vec<PathBuf>,
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

    #[error("invalid CEF runtime: {0}")]
    InvalidCefRuntime(#[from] crate::cef::CefError),
}

impl ResolvedDistribution {
    /// Validates that all declared contents actually exist on disk.
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
            if !resource.exists() {
                return Err(DistributionError::MissingResource(resource.clone()));
            }
        }

        Ok(())
    }

    fn validate_cef(&self) -> Result<(), DistributionError> {
        crate::cef::validate_cef_runtime(&self.cef_runtime)?;
        Ok(())
    }
}

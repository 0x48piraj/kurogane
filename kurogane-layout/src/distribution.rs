use std::path::{Path, PathBuf};
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
#[derive(Debug)]
pub struct ResolvedDistribution {
    pub metadata: AppMetadata,
    pub executable: PathBuf,
    pub frontend: Option<PathBuf>,
    pub cef_root: PathBuf,
    pub extra_resources: Vec<PathBuf>,
}

#[derive(Debug, Error)]
pub enum DistributionError {
    #[error("executable not found: {0}")]
    MissingExecutable(PathBuf),

    #[error("frontend directory not found: {0}")]
    MissingFrontend(PathBuf),

    #[error("frontend missing index.html at {0}")]
    MissingIndex(PathBuf),

    #[error("CEF root not found: {0}")]
    MissingCefRoot(PathBuf),

    #[error("extra resource not found: {0}")]
    MissingResource(PathBuf),

    #[error("required file missing from CEF: {0}")]
    MissingCefFile(&'static str),
}

impl ResolvedDistribution {
    /// Validates that all declared contents actually exist on disk.
    pub fn validate(&self) -> Result<(), DistributionError> {
        if !self.executable.exists() {
            return Err(DistributionError::MissingExecutable(
                self.executable.clone(),
            ));
        }

        if let Some(frontend) = &self.frontend {
            if !frontend.exists() {
                return Err(DistributionError::MissingFrontend(frontend.clone()));
            }

            let index = frontend.join("index.html");
            if !index.exists() {
                return Err(DistributionError::MissingIndex(index));
            }
        }

        if !self.cef_root.exists() {
            return Err(DistributionError::MissingCefRoot(self.cef_root.clone()));
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
        #[cfg(target_os = "windows")]
        {
            require_file(&self.cef_root, "libcef.dll")?;
        }

        #[cfg(target_os = "linux")]
        {
            require_file(&self.cef_root, "libcef.so")?;
        }

        Ok(())
    }
}

fn require_file(root: &Path, name: &'static str) -> Result<(), DistributionError> {
    if root.join(name).exists() {
        Ok(())
    } else {
        Err(DistributionError::MissingCefFile(name))
    }
}

use std::path::{Path, PathBuf};
use crate::error::ResolveError;

#[derive(Clone, Debug)]
pub struct CanonicalRoot(PathBuf);

impl CanonicalRoot {
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self, ResolveError> {
        let canonical = path.as_ref()
            .canonicalize()
            .map_err(ResolveError::Io)?;

        if !canonical.is_dir() {
            return Err(ResolveError::InvalidRoot(canonical));
        }

        Ok(Self(canonical))
    }

    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for CanonicalRoot {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

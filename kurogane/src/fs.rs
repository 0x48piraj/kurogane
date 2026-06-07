use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct CanonicalRoot(PathBuf);

impl CanonicalRoot {
    pub fn new<P: AsRef<Path>>(path: P) -> std::io::Result<Self> {
        let canonical = path.as_ref().canonicalize()?;

        if !canonical.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotADirectory,
                format!("{} is not a directory", canonical.display()),
            ));
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


#[test]
fn canonical_root_is_always_absolute() {
    let dir = tempfile::tempdir().unwrap();

    let root =
        CanonicalRoot::new(
            dir.path()
        )
        .unwrap();

    assert!(
        root.as_path().is_absolute()
    );
}

//! Internal frontend resolution logic.

use std::path::{Path, PathBuf};

use super::Source;

use crate::error::RuntimeError;
use crate::scheme::CanonicalRoot;

/// Result of frontend resolution.
#[derive(Debug)]
pub struct ResolvedFrontend {
    pub asset_root: Option<CanonicalRoot>,
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

            let root = CanonicalRoot::new(&dir)
                .map_err(|_| RuntimeError::InvalidAssetRoot(dir.clone()))?;

            Ok(ResolvedFrontend {
                asset_root: Some(root),
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


#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn tmp() -> TempDir {
        tempfile::tempdir().expect("failed to create temp dir")
    }

    // URL resolution tests

    #[test]
    fn resolve_url_returns_direct_url() {
        let source = Source::Url("http://localhost:3000".into());

        let result = resolve(&source).unwrap();

        assert_eq!(result.start_url, "http://localhost:3000");
        assert!(result.asset_root.is_none());
    }

    #[test]
    fn resolve_url_preserves_string_exactly() {
        let url = "https://example.com/app?foo=bar#section";
        let source = Source::Url(url.into());

        let result = resolve(&source).unwrap();

        assert_eq!(result.start_url, url);
    }

    // Path resolution tests

    #[test]
    fn resolve_absolute_path_success() {
        let dir = tmp();
        fs::write(dir.path().join("index.html"), b"<html></html>").unwrap();

        let source = Source::Path(dir.path().to_path_buf());

        let result = resolve(&source).unwrap();

        assert_eq!(result.start_url, APP_URL);

        let expected = dir.path().canonicalize().unwrap();
        assert_eq!(result.asset_root.unwrap(), expected);
    }

    #[test]
    fn resolve_relative_path_success() {
        let dir = tmp();
        fs::write(dir.path().join("index.html"), b"<html></html>").unwrap();

        let cwd = std::env::current_dir().unwrap();
        let relative = dir.path().strip_prefix(&cwd).unwrap_or(dir.path());

        let source = Source::Path(relative.to_path_buf());

        let result = resolve(&source).unwrap();

        assert_eq!(result.start_url, APP_URL);

        let expected = dir.path().canonicalize().unwrap();
        assert_eq!(result.asset_root.unwrap(), expected);
    }

    #[test]
    fn resolve_returns_canonicalized_path() {
        let dir = tmp();
        fs::write(dir.path().join("index.html"), b"ok").unwrap();

        let nested = dir.path().join(".");
        let source = Source::Path(nested);

        let result = resolve(&source).unwrap();

        let expected = dir.path().canonicalize().unwrap();
        assert_eq!(result.asset_root.unwrap(), expected);
    }

    // Validation failure tests

    #[test]
    fn resolve_fails_when_directory_missing() {
        let dir = tmp();
        let missing = dir.path().join("does_not_exist");

        let source = Source::Path(missing.clone());

        let err = resolve(&source).unwrap_err();

        match err {
            RuntimeError::AssetRootMissing(p) => {
                assert!(p.ends_with("does_not_exist"));
            }
            _ => panic!("expected AssetRootMissing"),
        }
    }

    #[test]
    fn resolve_fails_when_path_is_file() {
        let dir = tmp();
        let file = dir.path().join("file.txt");
        fs::write(&file, b"hello").unwrap();

        let source = Source::Path(file.clone());

        let err = resolve(&source).unwrap_err();

        match err {
            RuntimeError::InvalidAssetRoot(p) => {
                assert_eq!(p, file.canonicalize().unwrap());
            }
            _ => panic!("expected InvalidAssetRoot"),
        }
    }

    #[test]
    fn resolve_fails_when_index_missing() {
        let dir = tmp();

        let source = Source::Path(dir.path().to_path_buf());

        let err = resolve(&source).unwrap_err();

        match err {
            RuntimeError::AssetRootMissing(p) => {
                assert_eq!(p, dir.path().canonicalize().unwrap());
            }
            _ => panic!("expected AssetRootMissing"),
        }
    }

    // Edge cases

    #[test]
    fn resolve_fails_when_index_is_directory() {
        let dir = tmp();
        fs::create_dir(dir.path().join("index.html")).unwrap();

        let source = Source::Path(dir.path().to_path_buf());

        let err = resolve(&source).unwrap_err();

        match err {
            RuntimeError::AssetRootMissing(_) => {}
            _ => panic!("expected AssetRootMissing"),
        }
    }
}

#[cfg(test)]
mod property_tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn prop_valid_dirs_always_resolve(depth in 0usize..5) {
            let dir = tempfile::tempdir().unwrap();
            let mut path = dir.path().to_path_buf();

            // create nested structure
            for i in 0..depth {
                path = path.join(format!("dir{}", i));
            }

            std::fs::create_dir_all(&path).unwrap();

            // must contain index.html
            std::fs::write(path.join("index.html"), b"ok").unwrap();

            let source = Source::Path(path.clone());

            let result = resolve(&source).unwrap();

            let expected = path.canonicalize().unwrap();

            prop_assert_eq!(result.asset_root.unwrap(), expected);
            prop_assert_eq!(result.start_url, APP_URL);
        }
    }

    proptest! {
        #[test]
        fn prop_missing_index_always_fails(depth in 0usize..5) {
            let dir = tempfile::tempdir().unwrap();
            let mut path = dir.path().to_path_buf();

            for i in 0..depth {
                path = path.join(format!("dir{}", i));
            }

            std::fs::create_dir_all(&path).unwrap();

            // no index.html

            let source = Source::Path(path);

            let err = resolve(&source).unwrap_err();

            prop_assert!(matches!(err, RuntimeError::AssetRootMissing(_)));
        }
    }

    proptest! {
        #[test]
        fn prop_file_path_is_invalid_root(name in "[a-z]{1,8}") {
            let dir = tempfile::tempdir().unwrap();
            let file = dir.path().join(name);

            std::fs::write(&file, b"data").unwrap();

            let source = Source::Path(file);

            let err = resolve(&source).unwrap_err();

            prop_assert!(matches!(err, RuntimeError::InvalidAssetRoot(_)));
        }
    }

    proptest! {
        #[test]
        fn prop_paths_are_always_canonicalized(depth in 0usize..5) {
            let dir = tempfile::tempdir().unwrap();
            let mut path = dir.path().to_path_buf();

            for i in 0..depth {
                path = path.join(format!("dir{}", i));
            }

            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join("index.html"), b"ok").unwrap();

            // introduce weird path
            let weird = path.join(".").join("././");

            let source = Source::Path(weird);

            let result = resolve(&source).unwrap();

            let canonical = path.canonicalize().unwrap();

            prop_assert_eq!(result.asset_root.unwrap(), canonical);
        }
    }

    // URL inputs are always preserved
    proptest! {
        #[test]
        fn prop_url_is_identity(url in "https?://[a-z]{1,10}\\.com(/[a-z]{0,5})?") {
            let source = Source::Url(url.clone());

            let result = resolve(&source).unwrap();

            prop_assert_eq!(result.start_url, url);
            prop_assert!(result.asset_root.is_none());
        }
    }
}

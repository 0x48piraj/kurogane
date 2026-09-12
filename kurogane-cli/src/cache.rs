//! Persistent template cache.
//!
//! Caches git-backed templates for reuse across project generation.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// A template repository materialized on disk.
#[derive(Debug)]
pub struct Acquired {
    /// Directory containing the template checkout.
    pub path: PathBuf,
    /// Short commit id of the checkout, for display.
    pub commit: String,
}

/// Default location of the template cache.
pub fn templates_root() -> Result<PathBuf> {
    Ok(dirs::cache_dir()
        .context("Could not determine cache directory")?
        .join("kurogane")
        .join("templates"))
}

/// Cache entry directory for a git URL.
fn entry_dir(root: &Path, url: &str) -> PathBuf {
    let repo_name = url
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("template");
    let sanitized: String = repo_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();

    let mut hasher = DefaultHasher::new();
    url.hash(&mut hasher);
    let hash = format!("{:016x}", hasher.finish());

    root.join(format!("{sanitized}-{hash}"))
}

/// Short commit id of a checked-out repository.
fn head_commit(path: &Path) -> Result<String> {
    let repo = git2::Repository::open(path)?;
    let head = repo.head()?;
    let commit = head.peel_to_commit()?;
    Ok(commit
        .as_object()
        .short_id()?
        .as_str()
        .unwrap_or_default()
        .to_string())
}

/// Clone a git template into the persistent cache and reuse the cached copy.
///
/// Cache misses clone the default branch; cache hits reuse the existing
/// checkout without contacting the network. Corrupt or incomplete entries
/// are removed and re-cloned.
///
/// Filesystem paths are accepted for network-free local use and are not cached.
pub fn acquire(url: &str) -> Result<Acquired> {
    acquire_in(&templates_root()?, url)
}

pub fn acquire_in(root: &Path, url: &str) -> Result<Acquired> {
    let entry = entry_dir(root, url);

    if entry.exists() {
        match head_commit(&entry) {
            Ok(commit) => {
                return Ok(Acquired {
                    path: entry,
                    commit,
                });
            }
            Err(_) => {
                // Corrupt or partial entry; start over
                std::fs::remove_dir_all(&entry).with_context(|| {
                    format!(
                        "failed to remove corrupt template cache entry {}",
                        entry.display()
                    )
                })?;
            }
        }
    }

    std::fs::create_dir_all(root)
        .with_context(|| format!("failed to create directory {}", root.display()))?;

    let mut builder = git2::build::RepoBuilder::new();
    builder
        .clone(url, &entry)
        .with_context(|| format!("could not clone template from '{url}'"))?;

    let commit = head_commit(&entry)?;
    Ok(Acquired {
        path: entry,
        commit,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinct_urls_map_to_distinct_entries_with_stable_names() {
        let root = tempfile::tempdir().unwrap();
        let a = entry_dir(root.path(), "https://github.com/example/one");
        let b = entry_dir(root.path(), "https://github.com/example/two");
        let a_again = entry_dir(root.path(), "https://github.com/example/one");

        assert_ne!(a, b);
        assert_eq!(a, a_again);
        assert!(a.to_str().unwrap().contains("one-"));
    }
}

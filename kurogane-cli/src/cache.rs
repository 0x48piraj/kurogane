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

    std::fs::create_dir_all(root)?;

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

    fn fixture_repo(dir: &Path, file: &str) -> git2::Repository {
        let repo = git2::Repository::init(dir).unwrap();
        std::fs::write(dir.join(".gitattributes"), "* text eol=lf\n").unwrap();
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"tpl\"\nversion = \"0.0.0\"\n",
        )
        .unwrap();
        std::fs::write(dir.join(file), "fn main() {}\n").unwrap();

        let mut index = repo.index().unwrap();
        index
            .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
            .unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        {
            let tree = repo.find_tree(tree_id).unwrap();
            let sig = git2::Signature::now("Kurogane Tests", "tests@kurogane.invalid").unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
                .unwrap();
        }
        repo
    }

    #[test]
    fn first_acquire_clones_and_second_reuses_without_network() {
        let fixture = tempfile::tempdir().unwrap();
        fixture_repo(fixture.path(), "src/main.rs");
        let url = fixture.path().to_str().unwrap();

        let root = tempfile::tempdir().unwrap();

        let first = acquire_in(root.path(), url).unwrap();
        assert!(!first.commit.is_empty());
        assert!(first.path.join("Cargo.toml").exists());

        // A cache hit must not reflect changes to the source repository
        std::fs::write(fixture.path().join("src/main.rs"), "CHANGED").unwrap();

        let second = acquire_in(root.path(), url).unwrap();
        assert_eq!(first.path, second.path);
        assert_eq!(first.commit, second.commit);
        assert_eq!(
            std::fs::read_to_string(second.path.join("src/main.rs")).unwrap(),
            "fn main() {}\n"
        );
    }

    #[test]
    fn corrupt_entries_are_removed_and_recloned() {
        let fixture = tempfile::tempdir().unwrap();
        fixture_repo(fixture.path(), "src/main.rs");
        let url = fixture.path().to_str().unwrap();

        let root = tempfile::tempdir().unwrap();
        let first = acquire_in(root.path(), url).unwrap();

        // Corrupt; destroy the repository metadata
        std::fs::remove_dir_all(first.path.join(".git")).unwrap();

        let second = acquire_in(root.path(), url).unwrap();
        assert_eq!(
            std::fs::read_to_string(second.path.join("src/main.rs")).unwrap(),
            "fn main() {}\n"
        );
        assert!(second.path.join(".git").exists());
    }

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

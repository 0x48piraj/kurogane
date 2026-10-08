//! Snapshots of git templates.
//!
//! Each generation run clones a git template afresh and swaps the clone in
//! by renames, so no run reads a half-written snapshot. A run that cannot
//! fetch generates from the last snapshot.

use std::fs;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};

use crate::tui;

/// Returns the clone URL of the git repository `reference` names, by
/// cargo-generate's rules.
///
/// A host shorthand or URL names a repository, then an existing path names a
/// local template, then `owner/repo` names a GitHub repository. Anything else,
/// such as a cargo-generate favorite, is cargo-generate's to place.
pub(crate) fn git_url(reference: &str, cwd: &Path) -> Option<String> {
    if let Some(url) = shorthand_url(reference) {
        return Some(url);
    }
    if looks_like_url(reference) {
        return Some(reference.to_owned());
    }

    let path = Path::new(reference);
    if path.is_absolute() || cwd.join(path).is_dir() {
        return None;
    }

    github_owner_repo(reference).map(|owner_repo| format!("https://github.com/{owner_repo}.git"))
}

/// Returns the clone URL a `gh:`, `gl:`, `bb:` or `sr:` shorthand names.
fn shorthand_url(reference: &str) -> Option<String> {
    let (prefix, owner_repo) = (reference.get(..3)?, reference.get(3..)?);
    match prefix {
        "gh:" => Some(format!("https://github.com/{owner_repo}.git")),
        "gl:" => Some(format!("https://gitlab.com/{owner_repo}.git")),
        "bb:" => Some(format!("https://bitbucket.org/{owner_repo}.git")),
        "sr:" => Some(format!("https://git.sr.ht/~{owner_repo}")),
        _ => None,
    }
}

/// Returns whether `reference` is a URL or an scp-style `user@host:path`.
fn looks_like_url(reference: &str) -> bool {
    const SCHEMES: [&str; 5] = ["https://", "http://", "ssh://", "git://", "file://"];
    if SCHEMES.iter().any(|scheme| reference.starts_with(scheme)) {
        return true;
    }

    // In scp-style the colon comes before the first slash
    reference.find('@').is_some_and(|at| {
        let slash = reference.find('/').unwrap_or(reference.len());
        reference[at..]
            .find(':')
            .is_some_and(|colon| at + colon < slash)
    })
}

/// Returns `reference` when it is exactly `owner/repo`.
fn github_owner_repo(reference: &str) -> Option<&str> {
    let (owner, repo) = reference.split_once('/')?;
    let valid = |part: &str| {
        !part.is_empty()
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    };
    (valid(owner) && valid(repo) && !owner.starts_with('.')).then_some(reference)
}

/// Template snapshots under one directory.
pub(crate) struct Store {
    root: PathBuf,
}

impl Store {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Returns the snapshot of the git template at `url` after refreshing it
    /// through `fetch`, or the last snapshot when the refresh fails.
    ///
    /// `fetch` clones the template into the directory it is given.
    pub(crate) fn snapshot(
        &self,
        url: &str,
        fetch: impl FnOnce(&str, &Path) -> Result<()>,
    ) -> Result<PathBuf> {
        let snapshot = self.root.join(snapshot_key(url));

        match self.refresh(url, &snapshot, fetch) {
            Ok(()) => Ok(snapshot),
            Err(error) if snapshot.is_dir() => {
                tui::warn(&format!(
                    "Could not refresh the template; using the snapshot from {}",
                    age(&snapshot)
                ));
                tui::field("template", url);
                tui::field("reason", format!("{error:#}"));
                Ok(snapshot)
            }
            Err(error) => Err(error.context(format!("could not fetch the template {url}"))),
        }
    }

    /// Fetches `url` into a staging directory and swaps it in for `snapshot`.
    fn refresh(
        &self,
        url: &str,
        snapshot: &Path,
        fetch: impl FnOnce(&str, &Path) -> Result<()>,
    ) -> Result<()> {
        let staging = self.scratch(".staging")?;
        if let Err(error) = fetch(url, &staging) {
            let _ = fs::remove_dir_all(&staging);
            return Err(error);
        }

        let parent = snapshot.parent().unwrap_or(&self.root);
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory {}", parent.display()))?;

        let trash = self.scratch(".trash")?;
        if snapshot.exists() {
            fs::rename(snapshot, &trash)
                .with_context(|| format!("failed to move {} aside", snapshot.display()))?;
        }

        // A concurrent run that swapped in first keeps its snapshot
        if let Err(error) = fs::rename(&staging, snapshot) {
            let _ = fs::remove_dir_all(&staging);
            if !snapshot.is_dir() {
                return Err(error)
                    .with_context(|| format!("failed to place {}", snapshot.display()));
            }
        }

        // Old snapshots and the leftovers of interrupted swaps
        let _ = fs::remove_dir_all(self.root.join(".trash"));
        Ok(())
    }

    /// Returns a path in the store's `kind` folder that no other run uses.
    fn scratch(&self, kind: &str) -> Result<PathBuf> {
        let dir = self.root.join(kind);
        fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create directory {}", dir.display()))?;
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        Ok(dir.join(format!("{}-{nanos}", std::process::id())))
    }
}

/// Returns the snapshot directory of `url` within the store, its host and
/// path with each part limited to letters, digits, `.`, `-` and `_`.
fn snapshot_key(url: &str) -> PathBuf {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let rest = rest.split_once('@').map_or(rest, |(_, rest)| rest);
    let rest = rest.strip_suffix(".git").unwrap_or(rest);

    rest.split(['/', ':'])
        .filter(|part| !part.is_empty() && *part != "." && *part != "..")
        .map(|part| {
            part.chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                        c
                    } else {
                        '_'
                    }
                })
                .collect::<String>()
        })
        .collect()
}

/// Returns how long ago `snapshot` was taken, in words.
fn age(snapshot: &Path) -> String {
    let elapsed = fs::metadata(snapshot)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok());
    let Some(elapsed) = elapsed else {
        return "an unknown time".to_owned();
    };

    let minutes = elapsed.as_secs() / 60;
    match (minutes, minutes / 60, minutes / (60 * 24)) {
        (0, _, _) => "less than a minute ago".to_owned(),
        (minutes, 0, _) => format!("{minutes} minutes ago"),
        (_, hours, 0) => format!("{hours} hours ago"),
        (_, _, days) => format!("{days} days ago"),
    }
}

/// Clones the git template at `url` into `into` with its submodules, keeping
/// the files alone.
///
/// The clone takes the settings cargo-generate uses. HTTP clones are shallow;
/// the user's credential helpers, SSH agent and keys authenticate; prompts
/// appear only when `interactive` holds and a terminal is attached.
pub(crate) fn clone(url: &str, into: &Path, interactive: bool) -> Result<()> {
    let prompts = interactive && std::io::stdin().is_terminal() && std::io::stderr().is_terminal();
    let authenticator = auth_git2::GitAuthenticator::default()
        .try_password_prompt(if prompts { 3 } else { 0 })
        .prompt_ssh_key_password(prompts);
    let config = git2::Config::open_default().map_err(git_error)?;
    let fetch_options = || {
        let mut callbacks = git2::RemoteCallbacks::new();
        callbacks.credentials(authenticator.credentials(&config));
        let mut proxy = git2::ProxyOptions::new();
        proxy.auto();
        let mut options = git2::FetchOptions::new();
        options.remote_callbacks(callbacks).proxy_options(proxy);
        options
    };

    let mut options = fetch_options();
    if url.starts_with("https://") || url.starts_with("http://") {
        options.depth(1);
    }
    let repo = git2::build::RepoBuilder::new()
        .fetch_options(options)
        .clone(url, into)
        .map_err(git_error)?;

    for mut submodule in repo.submodules().map_err(git_error)? {
        let mut update = git2::SubmoduleUpdateOptions::new();
        update.fetch(fetch_options());
        submodule
            .update(true, Some(&mut update))
            .map_err(git_error)?;
    }
    drop(repo);

    let git = into.join(".git");
    fs::remove_dir_all(&git).with_context(|| format!("failed to remove {}", git.display()))
}

/// Returns the message of a libgit2 error without its class and code.
fn git_error(error: git2::Error) -> anyhow::Error {
    anyhow::anyhow!("{}", error.message().trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_name_repositories_as_cargo_generate_reads_them() {
        let cwd = tempfile::tempdir().unwrap();
        fs::create_dir_all(cwd.path().join("local/template")).unwrap();

        for (reference, url) in [
            ("gh:o/r", Some("https://github.com/o/r.git")),
            ("gl:o/r", Some("https://gitlab.com/o/r.git")),
            ("bb:o/r", Some("https://bitbucket.org/o/r.git")),
            ("sr:o/r", Some("https://git.sr.ht/~o/r")),
            ("https://x.org/o/r", Some("https://x.org/o/r")),
            ("file:///srv/r", Some("file:///srv/r")),
            ("git@x.org:o/r.git", Some("git@x.org:o/r.git")),
            ("o/r", Some("https://github.com/o/r.git")),
            ("local/template", None),
            ("favorite", None),
            (".o/r", None),
        ] {
            assert_eq!(
                git_url(reference, cwd.path()).as_deref(),
                url,
                "{reference}"
            );
        }
    }

    #[test]
    fn a_snapshot_is_named_by_host_and_path() {
        for (url, key) in [
            ("https://github.com/o/r.git", "github.com/o/r"),
            ("git@github.com:o/r.git", "github.com/o/r"),
            ("https://x.org:8443/a b/r", "x.org/8443/a_b/r"),
            ("https://x.org/../r", "x.org/r"),
        ] {
            let expected: PathBuf = key.split('/').collect();
            assert_eq!(snapshot_key(url), expected, "{url}");
        }
    }

    fn fetch_writing(contents: &'static str) -> impl FnOnce(&str, &Path) -> Result<()> {
        move |_, into| {
            fs::create_dir_all(into)?;
            fs::write(into.join("Cargo.toml"), contents)?;
            Ok(())
        }
    }

    fn fetch_failing(_: &str, _: &Path) -> Result<()> {
        anyhow::bail!("offline")
    }

    #[test]
    fn each_run_replaces_the_snapshot_whole() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::new(root.path().to_path_buf());
        let url = "https://github.com/o/r.git";

        let first = store.snapshot(url, fetch_writing("first")).unwrap();
        fs::write(first.join("stale.txt"), "").unwrap();
        let second = store.snapshot(url, fetch_writing("second")).unwrap();

        assert_eq!(first, second);
        assert_eq!(
            fs::read_to_string(second.join("Cargo.toml")).unwrap(),
            "second"
        );
        assert!(
            !second.join("stale.txt").exists(),
            "nothing of the old snapshot stays"
        );
        assert!(
            !root.path().join(".trash").exists(),
            "the old snapshot is removed"
        );
    }

    #[test]
    fn a_run_that_cannot_fetch_uses_the_last_snapshot() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::new(root.path().to_path_buf());
        let url = "https://github.com/o/r.git";

        store.snapshot(url, fetch_writing("kept")).unwrap();
        let snapshot = store.snapshot(url, fetch_failing).unwrap();

        assert_eq!(
            fs::read_to_string(snapshot.join("Cargo.toml")).unwrap(),
            "kept"
        );
        let staged = fs::read_dir(root.path().join(".staging")).unwrap().count();
        assert_eq!(staged, 0, "a failed fetch leaves nothing staged");
    }

    #[test]
    fn a_first_run_that_cannot_fetch_fails() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::new(root.path().to_path_buf());

        let error = store
            .snapshot("https://github.com/o/r.git", fetch_failing)
            .unwrap_err();
        assert!(format!("{error:#}").contains("offline"), "{error:#}");
    }

    #[test]
    fn a_clone_keeps_the_files_alone() {
        let dir = tempfile::tempdir().unwrap();
        let origin = dir.path().join("origin");
        let repo = git2::Repository::init(&origin).unwrap();
        fs::write(origin.join("Cargo.toml"), "template").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("Cargo.toml")).unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let author = git2::Signature::now("t", "t@example.com").unwrap();
        repo.commit(Some("HEAD"), &author, &author, "template", &tree, &[])
            .unwrap();

        let into = dir.path().join("clone");
        clone(origin.to_str().unwrap(), &into, false).unwrap();

        assert_eq!(
            fs::read_to_string(into.join("Cargo.toml")).unwrap(),
            "template"
        );
        assert!(
            !into.join(".git").exists(),
            "the snapshot holds no repository"
        );
    }
}

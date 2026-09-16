//! [`Scope`] defines a resource boundary through literal allow roots and
//! root-relative deny rules. Deny always wins.
//!
//! Allow roots are literal directories anchored on a [`SafeRoot`]. A
//! non-recursive root covers itself and its immediate children; a recursive
//! root covers its entire subtree. Requests may match a root through either
//! its configured path or its kernel-reported canonical path.
//!
//! Deny rules are relative to the scope's roots. `deny_path` defines a literal
//! subtree and must lie beneath a root; `deny_glob` is evaluated beneath every
//! root. Both are matched against [`Location`]s, so the same rules apply to
//! the requested path and the location reported for the opened object.
//!
//! [`Scope`] does not open objects and has no knowledge of origins.

use std::io;
use std::path::{Path, PathBuf};

use crate::capability::error::FsConfigError;
use crate::capability::path::{Key, Location, Name, RelPath};
use crate::capability::safe::SafeRoot;

mod glob;

pub(crate) use glob::{Glob, Residual};

/// How deep beneath its root an allow root reaches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Extent {
    /// The root itself and its immediate children.
    Children,
    /// The whole subtree.
    Subtree,
}

impl Extent {
    fn admits(self, depth: usize) -> bool {
        depth <= 1 || self == Extent::Subtree
    }
}

/// Declares one scope inside [`FilesystemBuilder::scope`](crate::capability::FilesystemBuilder::scope).
///
/// Methods only record; roots are opened and patterns checked when the
/// `Filesystem` is built.
#[derive(Debug, Default)]
pub struct ScopeBuilder {
    roots: Vec<(PathBuf, Extent)>,
    deny_paths: Vec<PathBuf>,
    deny_globs: Vec<String>,
}

impl ScopeBuilder {
    /// Allows the directory itself and its immediate children.
    pub fn allow_directory(&mut self, path: impl AsRef<Path>) -> &mut Self {
        self.roots
            .push((path.as_ref().to_path_buf(), Extent::Children));
        self
    }

    /// Allows the directory and its whole subtree.
    pub fn allow_directory_recursive(&mut self, path: impl AsRef<Path>) -> &mut Self {
        self.roots
            .push((path.as_ref().to_path_buf(), Extent::Subtree));
        self
    }

    /// Denies a literal path and everything beneath it. The path is never a
    /// pattern, whatever characters it contains, and must lie under one of
    /// this scope's allow roots.
    pub fn deny_path(&mut self, path: impl AsRef<Path>) -> &mut Self {
        self.deny_paths.push(path.as_ref().to_path_buf());
        self
    }

    /// Denies what `pattern` matches beneath every allow root of this scope
    /// (`**/.git`, `**/*.key`). Patterns use `/` on every platform and `\`
    /// as the escape; a matched directory is denied with its subtree.
    pub fn deny_glob(&mut self, pattern: impl Into<String>) -> &mut Self {
        self.deny_globs.push(pattern.into());
        self
    }

    pub(crate) fn build(self, scope: &str) -> Result<Scope, FsConfigError> {
        let mut globs = Vec::with_capacity(self.deny_globs.len());
        for pattern in self.deny_globs {
            match Glob::parse(&pattern) {
                Ok(glob) => globs.push(glob),
                Err(reason) => {
                    return Err(FsConfigError::InvalidGlob {
                        scope: scope.to_owned(),
                        pattern,
                        reason,
                    });
                }
            }
        }

        let mut roots = Vec::with_capacity(self.roots.len());
        for (path, extent) in self.roots {
            roots.push(Root::open(path, extent, &globs)?);
        }

        for path in self.deny_paths {
            let location = deny_location(&path);
            let mut matched = false;
            for root in &mut roots {
                if let Some(rel) = location.as_ref().and_then(|l| l.strip(&root.canonical)) {
                    root.deny.push(DenyRule::Subtree(rel.to_vec()));
                    matched = true;
                }
            }
            if !matched {
                return Err(FsConfigError::DenyOutsideRoots {
                    scope: scope.to_owned(),
                    path,
                });
            }
        }

        Ok(Scope { roots })
    }
}

/// A built scope.
pub(crate) struct Scope {
    roots: Vec<Root>,
}

impl Scope {
    pub(crate) fn roots(&self) -> &[Root] {
        &self.roots
    }
}

/// One allow root.
pub(crate) struct Root {
    safe: SafeRoot,
    extent: Extent,
    canonical: Location,
    aliases: Vec<Location>,
    deny: Vec<DenyRule>,
}

enum DenyRule {
    /// A literal subtree, relative to the root's canonical location.
    Subtree(Vec<Key>),
    Glob(Glob),
}

impl DenyRule {
    fn covers(&self, rel: &[Key]) -> bool {
        match self {
            DenyRule::Subtree(prefix) => rel.starts_with(prefix),
            DenyRule::Glob(glob) => glob.covers(rel),
        }
    }
}

impl Root {
    fn open(path: PathBuf, extent: Extent, globs: &[Glob]) -> Result<Root, FsConfigError> {
        let safe = match SafeRoot::open(&path) {
            Ok(safe) => safe,
            Err(source) => return Err(FsConfigError::Root { path, source }),
        };
        let Some(canonical) = Location::parse(safe.canonical()) else {
            let source = io::Error::other("the root has no absolute location");
            return Err(FsConfigError::Root { path, source });
        };
        let mut aliases = vec![canonical.clone()];
        if let Some(configured) = std::path::absolute(&path)
            .ok()
            .and_then(|p| Location::parse(&p))
            && configured != canonical
        {
            aliases.push(configured);
        }
        Ok(Root {
            safe,
            extent,
            canonical,
            aliases,
            deny: globs.iter().cloned().map(DenyRule::Glob).collect(),
        })
    }

    pub(crate) fn safe(&self) -> &SafeRoot {
        &self.safe
    }

    pub(crate) fn canonical(&self) -> &Location {
        &self.canonical
    }

    /// Anchors an absolute request at this root; the relative tail and how
    /// specific the matching alias is (longer wins), if the request lies
    /// under an alias within the root's extent.
    pub(crate) fn anchor(&self, volume: &Key, names: &[Name]) -> Option<(usize, RelPath)> {
        self.aliases
            .iter()
            .filter_map(|alias| Some((alias.keys().len(), alias.relative(volume, names)?)))
            .filter(|(_, rel)| self.admits(rel))
            .max_by_key(|(specificity, _)| *specificity)
    }

    /// Whether a relative tail lies within the root's extent.
    pub(crate) fn admits(&self, rel: &RelPath) -> bool {
        self.extent.admits(rel.len())
    }

    /// Whether a deny rule of this root covers `location`. Locations outside
    /// the root are not this root's to judge.
    pub(crate) fn denies(&self, location: &Location) -> bool {
        let Some(rel) = location.strip(&self.canonical) else {
            return false;
        };
        self.deny.iter().any(|rule| rule.covers(rel))
    }

    /// Whether this root's extent reaches `location`.
    pub(crate) fn reaches(&self, location: &Location) -> bool {
        location
            .strip(&self.canonical)
            .is_some_and(|rel| self.extent.admits(rel.len()))
    }

    /// Whether this root reaches every descendant of `location`.
    pub(crate) fn reaches_beneath(&self, location: &Location) -> bool {
        self.extent == Extent::Subtree && location.strip(&self.canonical).is_some()
    }

    /// The deny subtrees lying strictly beneath `location`, each as the
    /// suffix that remains below it: what a move of `location` carries.
    pub(crate) fn enclosed_denies<'a>(&'a self, location: &Location) -> Vec<&'a [Key]> {
        let Some(rel) = location.strip(&self.canonical) else {
            return Vec::new();
        };
        self.deny
            .iter()
            .filter_map(|rule| match rule {
                DenyRule::Subtree(prefix)
                    if prefix.len() > rel.len() && prefix.starts_with(rel) =>
                {
                    Some(&prefix[rel.len()..])
                }
                DenyRule::Subtree(_) | DenyRule::Glob(_) => None,
            })
            .collect()
    }

    /// Each deny glob of this root with what it still requires beneath
    /// `location`; nothing when `location` is not under this root.
    pub(crate) fn glob_residuals(&self, location: &Location) -> Vec<(&Glob, Residual)> {
        let Some(rel) = location.strip(&self.canonical) else {
            return Vec::new();
        };
        self.deny
            .iter()
            .filter_map(|rule| match rule {
                DenyRule::Glob(glob) => Some((glob, glob.residual(rel))),
                DenyRule::Subtree(_) => None,
            })
            .collect()
    }
}

/// The canonical location of a deny path; its longest existing ancestor is
/// canonicalized (resolving the same links and short names the roots were
/// resolved through) and the missing tail appended.
fn deny_location(path: &Path) -> Option<Location> {
    let absolute = std::path::absolute(path).ok()?;
    let mut tail = Vec::new();
    let mut current = absolute.as_path();
    loop {
        if let Ok(canonical) = current.canonicalize() {
            let base = Location::parse(&canonical)?;
            return Some(base.join(tail.iter().rev()));
        }
        tail.push(Key::of(current.file_name()?));
        current = current.parent()?;
    }
}

#[cfg(all(test, any(target_os = "linux", windows, target_os = "macos")))]
mod tests {
    use super::*;

    use crate::capability::path::{parse_request, Request};

    fn build(f: impl FnOnce(&mut ScopeBuilder)) -> Result<Scope, FsConfigError> {
        let mut builder = ScopeBuilder::default();
        f(&mut builder);
        builder.build("test")
    }

    fn anchor(scope: &Scope, path: &Path) -> Option<RelPath> {
        let Ok(Request::Absolute { volume, names }) = parse_request(path) else {
            return None;
        };
        scope.roots()[0].anchor(&volume, &names).map(|(_, rel)| rel)
    }

    fn denied(scope: &Scope, path: &Path) -> bool {
        let root = &scope.roots()[0];
        let rel = anchor(scope, path).expect("path is under the root");
        root.denies(&root.canonical().join(rel.names().iter().map(Name::key)))
    }

    #[test]
    fn extent_limits_depth_for_non_recursive_roots() {
        let d = tempfile::tempdir().unwrap();
        let scope = build(|s| {
            s.allow_directory(d.path());
        })
        .unwrap();
        assert_eq!(anchor(&scope, d.path()).map(|r| r.len()), Some(0));
        assert_eq!(
            anchor(&scope, &d.path().join("a.txt")).map(|r| r.len()),
            Some(1)
        );
        assert!(anchor(&scope, &d.path().join("sub").join("a.txt")).is_none());
    }

    #[test]
    fn recursive_roots_reach_any_depth_and_never_a_sibling() {
        let d = tempfile::tempdir().unwrap();
        let scope = build(|s| {
            s.allow_directory_recursive(d.path());
        })
        .unwrap();
        assert_eq!(
            anchor(&scope, &d.path().join("x/y/z.txt")).map(|r| r.len()),
            Some(3)
        );
        let mut sibling = d.path().as_os_str().to_owned();
        sibling.push("-evil");
        assert!(anchor(&scope, Path::new(&sibling)).is_none());
    }

    #[test]
    fn deny_path_covers_its_subtree_on_component_boundaries() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("secret")).unwrap();
        let scope = build(|s| {
            s.allow_directory_recursive(d.path());
            s.deny_path(d.path().join("secret"));
        })
        .unwrap();
        assert!(denied(&scope, &d.path().join("secret")));
        assert!(denied(&scope, &d.path().join("secret/x/y.txt")));
        assert!(!denied(&scope, &d.path().join("secrets")));
    }

    #[test]
    fn deny_path_may_name_a_missing_entry() {
        let d = tempfile::tempdir().unwrap();
        let scope = build(|s| {
            s.allow_directory_recursive(d.path());
            s.deny_path(d.path().join("later"));
        })
        .unwrap();
        assert!(denied(&scope, &d.path().join("later/file")));
    }

    #[test]
    fn deny_path_with_glob_characters_stays_literal() {
        let d = tempfile::tempdir().unwrap();
        let scope = build(|s| {
            s.allow_directory_recursive(d.path());
            s.deny_path(d.path().join("[1]"));
        })
        .unwrap();
        assert!(denied(&scope, &d.path().join("[1]")));
        assert!(!denied(&scope, &d.path().join("1")));
    }

    #[test]
    fn deny_glob_applies_beneath_each_root() {
        let d = tempfile::tempdir().unwrap();
        let scope = build(|s| {
            s.allow_directory_recursive(d.path());
            s.deny_glob("**/*.key");
        })
        .unwrap();
        assert!(denied(&scope, &d.path().join("a/b/secret.key")));
        assert!(!denied(&scope, &d.path().join("a/b/public.txt")));
    }

    #[test]
    fn configuration_errors_are_reported() {
        let d = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let err = build(|s| {
            s.allow_directory(d.path());
            s.deny_path(outside.path());
        });
        assert!(matches!(err, Err(FsConfigError::DenyOutsideRoots { .. })));

        let err = build(|s| {
            s.allow_directory(d.path());
            s.deny_glob("/abs/*.key");
        });
        assert!(matches!(err, Err(FsConfigError::InvalidGlob { .. })));

        let err = build(|s| {
            s.allow_directory(d.path().join("missing"));
        });
        assert!(matches!(err, Err(FsConfigError::Root { .. })));
    }

    #[cfg(windows)]
    #[test]
    fn windows_requests_match_roots_case_insensitively() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("Secret")).unwrap();
        let scope = build(|s| {
            s.allow_directory_recursive(d.path());
            s.deny_path(d.path().join("Secret"));
        })
        .unwrap();
        let upper = PathBuf::from(d.path().as_os_str().to_ascii_uppercase());
        assert!(anchor(&scope, &upper.join("file.txt")).is_some());
        assert!(denied(&scope, &upper.join("SECRET").join("x")));
        assert_eq!(
            Key::of(std::ffi::OsStr::new("a")),
            Key::of(std::ffi::OsStr::new("A"))
        );
    }
}

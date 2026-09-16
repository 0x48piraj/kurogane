//! Authorized filesystem access for a single origin.
//!
//! Each operation is authorized in the following order:
//!
//! 1. The request must parse; otherwise [`FsError::InvalidPath`] is returned.
//! 2. The origin must hold a grant for the requested capability; otherwise
//!    [`FsError::CapabilityDenied`] is returned.
//! 3. The most specific allowed root covering the path is selected. Relative
//!    paths require exactly one root for the origin.
//! 4. No scope held by the origin may deny the path;
//!    [`Denial::DenyRule`] takes precedence across grants.
//! 5. The object is opened beneath the selected root handle and its
//!    kernel-reported location is checked against the deny rules again before
//!    it is read, written or modified. [`Denial::ObjectLocation`] catches
//!    aliases such as 8.3 names and case variants on Windows.
//!
//! [`AuthorizedFs`] is the typed authority for an origin and can only be
//! minted by [`Filesystem::authorize`]. [`Scope`] defines the allowed roots
//! and deny rules; [`SafeRoot`] and [`Dir`] provide kernel-confined access.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::acl::Origin;
use crate::capability::error::{Denial, FsConfigError, FsError};
use crate::capability::path::{parse_request, Key, Location, Name, RelPath, Request};
use crate::capability::policy::FsAccess;
use crate::capability::safe::{self, Create, Dir, DirEntry, EntryKind};
use crate::capability::scope::{Residual, Root, Scope, ScopeBuilder};

/// The filesystem capability configuration: named scopes and the grants that
/// bind origins to them.
///
/// ```no_run
/// use kurogane::Origin;
/// use kurogane::capability::{Filesystem, FsAccess};
///
/// let mut builder = Filesystem::builder();
/// let notes = builder.scope("notes", |scope| {
///     scope.allow_directory_recursive("/path/to/notes");
///     scope.deny_path("/path/to/notes/secrets");
///     scope.deny_glob("**/.git");
/// });
/// builder.grant(Origin::parse("app://app")?, notes, FsAccess::READ | FsAccess::LIST);
/// let fs = builder.build()?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct Filesystem {
    scopes: Vec<Scope>,
    grants: Vec<Grant>,
    max_file_size: u64,
    allow_hard_links: bool,
    /// Every capability each origin holds, across its grants.
    access: HashMap<Origin, FsAccess>,
}

struct Grant {
    origin: Origin,
    scope: usize,
    access: FsAccess,
}

/// Handle to a scope declared on a [`FilesystemBuilder`]. Grants name scopes
/// by handle, so a grant cannot reference a scope that was never declared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScopeId {
    builder: u64,
    index: usize,
}

/// Builder for [`Filesystem`].
pub struct FilesystemBuilder {
    id: u64,
    scopes: Vec<(String, ScopeBuilder)>,
    grants: Vec<(Origin, ScopeId, FsAccess)>,
    max_file_size: u64,
    allow_hard_links: bool,
}

impl FilesystemBuilder {
    fn new() -> Self {
        // Distinguishes builders so a ScopeId cannot cross between them.
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        FilesystemBuilder {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            scopes: Vec::new(),
            grants: Vec::new(),
            max_file_size: Filesystem::DEFAULT_MAX_FILE_SIZE,
            allow_hard_links: false,
        }
    }

    /// Permits regular files that have other names (hard links). By default
    /// such a file is refused (`PATH_DENIED`) by every operation that reads,
    /// writes, copies or measures its contents; its other names may lie
    /// outside every root or under a deny rule, and no platform can tell the
    /// location re-check which. Enable only when the roots hold multiply
    /// linked files by design (a pnpm store, a local `git clone`) and nobody
    /// untrusted can create links in them. Listing, removing and renaming a
    /// link's name are unaffected either way.
    pub fn allow_hard_links(&mut self, allow: bool) -> &mut Self {
        self.allow_hard_links = allow;
        self
    }

    /// Caps the bytes `read_file` returns and `write_file` accepts
    /// ([`FsError::TooLarge`] beyond it). A whole file travels in one IPC
    /// message and is held in memory by both processes, so the cap bounds
    /// what one renderer call can cost until file transfers stream.
    /// Defaults to [`Filesystem::DEFAULT_MAX_FILE_SIZE`]. `copy_file`
    /// stays in the browser process and is not capped.
    pub fn max_file_size(&mut self, bytes: u64) -> &mut Self {
        self.max_file_size = bytes;
        self
    }

    /// Declares a scope and returns its handle. `name` labels configuration
    /// errors only.
    pub fn scope(
        &mut self,
        name: impl Into<String>,
        declare: impl FnOnce(&mut ScopeBuilder),
    ) -> ScopeId {
        let mut scope = ScopeBuilder::default();
        declare(&mut scope);
        self.scopes.push((name.into(), scope));
        ScopeId {
            builder: self.id,
            index: self.scopes.len() - 1,
        }
    }

    /// Grants `access` over `scope` to `origin`, matched exactly. Several
    /// grants to one origin combine.
    pub fn grant(&mut self, origin: Origin, scope: ScopeId, access: FsAccess) -> &mut Self {
        self.grants.push((origin, scope, access));
        self
    }

    /// Opens every allow root and checks every rule.
    ///
    /// # Errors
    ///
    /// Returns [`FsConfigError`] for a scope handle from another builder, a
    /// grant to the opaque origin, a malformed deny glob, a deny path outside
    /// its scope's roots, or an allow root that cannot be opened as a
    /// directory (including every root on platforms without a safe-open
    /// backend).
    pub fn build(self) -> Result<Filesystem, FsConfigError> {
        let mut grants = Vec::with_capacity(self.grants.len());
        for (origin, scope, access) in self.grants {
            if scope.builder != self.id {
                return Err(FsConfigError::ForeignScope);
            }
            if origin.is_opaque() {
                return Err(FsConfigError::OpaqueOrigin);
            }
            grants.push(Grant {
                origin,
                scope: scope.index,
                access,
            });
        }

        let mut scopes = Vec::with_capacity(self.scopes.len());
        for (name, scope) in self.scopes {
            scopes.push(scope.build(&name)?);
        }
        let mut access: HashMap<Origin, FsAccess> = HashMap::new();
        for grant in &grants {
            *access.entry(grant.origin.clone()).or_default() |= grant.access;
        }
        Ok(Filesystem {
            scopes,
            grants,
            max_file_size: self.max_file_size,
            allow_hard_links: self.allow_hard_links,
            access,
        })
    }
}

impl Default for FilesystemBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl Filesystem {
    /// The default of [`FilesystemBuilder::max_file_size`]: 64 MiB.
    pub const DEFAULT_MAX_FILE_SIZE: u64 = 64 * 1024 * 1024;

    pub fn builder() -> FilesystemBuilder {
        FilesystemBuilder::new()
    }

    /// Every capability `origin` holds across its grants; `NONE` without one.
    /// Lets a request be refused before it is copied or queued.
    pub(crate) fn access_of(&self, origin: &Origin) -> FsAccess {
        self.access.get(origin).copied().unwrap_or(FsAccess::NONE)
    }

    /// The transfer limit of [`FilesystemBuilder::max_file_size`].
    pub(crate) fn max_file_size(&self) -> u64 {
        self.max_file_size
    }

    /// Mints the typed authority for `origin`, or `None` when it holds no
    /// grant. This is the only origin-dependent decision in the layer.
    pub fn authorize(&self, origin: &Origin) -> Option<AuthorizedFs<'_>> {
        let grants: Vec<_> = self
            .grants
            .iter()
            .filter(|grant| &grant.origin == origin)
            .map(|grant| (&self.scopes[grant.scope], grant.access))
            .collect();
        (!grants.is_empty()).then_some(AuthorizedFs {
            grants,
            max_file_size: self.max_file_size,
            allow_hard_links: self.allow_hard_links,
        })
    }
}

/// The authority of one origin. Every operation runs the decision rules in
/// the module documentation; nothing else reaches the filesystem for a
/// renderer.
pub struct AuthorizedFs<'a> {
    grants: Vec<(&'a Scope, FsAccess)>,
    max_file_size: u64,
    allow_hard_links: bool,
}

/// A request resolved against one root. Not an authority by itself.
struct Target<'a> {
    root: &'a Root,
    rel: RelPath,
}

impl Target<'_> {
    fn split(&self) -> Result<(RelPath, &Name), FsError> {
        self.rel.split_leaf().ok_or(FsError::InvalidPath(
            "an allowed root itself cannot be created, removed or renamed",
        ))
    }
}

impl<'a> AuthorizedFs<'a> {
    /// Every capability the origin holds, across its grants.
    pub fn access(&self) -> FsAccess {
        self.grants
            .iter()
            .fold(FsAccess::NONE, |all, (_, access)| all | *access)
    }

    /// `fs.read_file`: READ. Regular files only, up to the transfer limit.
    pub fn read_file(&self, path: &Path) -> Result<Vec<u8>, FsError> {
        let target = self.resolve(FsAccess::READ, path)?;
        let file = target.root.safe().open_file(&target.rel)?;
        self.verify(target.root, &file)?;
        self.single_link(&file)?;
        let limit = self.max_file_size;
        if file.metadata()?.len() > limit {
            return Err(FsError::TooLarge { limit });
        }
        // The file can grow after the check: read one byte past the limit
        // to notice, never more
        let mut contents = Vec::new();
        (&file)
            .take(limit.saturating_add(1))
            .read_to_end(&mut contents)?;
        if self.exceeds_limit(&contents) {
            return Err(FsError::TooLarge { limit });
        }
        Ok(contents)
    }

    /// `fs.write_file`: WRITE to replace an existing file, CREATE to make a
    /// new one. The object decides which applies. Contents over the transfer
    /// limit are refused before anything is opened.
    pub fn write_file(&self, path: &Path, contents: &[u8]) -> Result<(), FsError> {
        if self.exceeds_limit(contents) {
            return Err(FsError::TooLarge {
                limit: self.max_file_size,
            });
        }
        let mut file = match self.open_existing(path) {
            Ok(file) => file,
            Err(FsError::Io(e)) if e.kind() == io::ErrorKind::NotFound => {
                match self.create_new(path) {
                    // Lost a creation race; the file exists now
                    Err(FsError::Io(e)) if e.kind() == io::ErrorKind::AlreadyExists => {
                        self.open_existing(path)?
                    }
                    created => created?,
                }
            }
            Err(denied @ (FsError::CapabilityDenied | FsError::PathDenied(_))) => {
                match self.create_new(path) {
                    // The file exists and replacing it needs WRITE, which was denied
                    Err(FsError::Io(e)) if e.kind() == io::ErrorKind::AlreadyExists => {
                        return Err(denied);
                    }
                    Err(FsError::CapabilityDenied) => return Err(denied),
                    created => created?,
                }
            }
            Err(e) => return Err(e),
        };
        file.write_all(contents)?;
        Ok(())
    }

    /// `fs.create_dir`: CREATE. One level; the parent must exist.
    pub fn create_dir(&self, path: &Path) -> Result<(), FsError> {
        let target = self.resolve(FsAccess::CREATE, path)?;
        let (parent, leaf) = target.split()?;
        let dir = target.root.safe().open_dir(&parent)?;
        self.verify_child(target.root, &dir, leaf)?;
        dir.create_dir(leaf)
    }

    /// `fs.remove_file`: DELETE.
    pub fn remove_file(&self, path: &Path) -> Result<(), FsError> {
        self.remove(path, EntryKind::File)
    }

    /// `fs.remove_dir`: DELETE. The directory must be empty.
    pub fn remove_dir(&self, path: &Path) -> Result<(), FsError> {
        self.remove(path, EntryKind::Dir)
    }

    /// `fs.rename_file`: RENAME on source and destination, each authorized
    /// independently. An existing destination is replaced.
    ///
    /// Every rule is a function of where content lives, so a move is also
    /// judged as a whole: it may not carry content out from under a deny rule
    /// (`PATH_DENIED`), nor to a place where the origin holds more access over
    /// it than it held at the source (`PATH_DENIED`).
    pub fn rename_file(&self, from: &Path, to: &Path) -> Result<(), FsError> {
        let source = self.resolve(FsAccess::RENAME, from)?;
        let destination = self.resolve(FsAccess::RENAME, to)?;
        let (source_parent, source_leaf) = source.split()?;
        let (destination_parent, destination_leaf) = destination.split()?;
        let source_dir = source.root.safe().open_dir(&source_parent)?;
        let destination_dir = destination.root.safe().open_dir(&destination_parent)?;

        let entry = source_dir.entry(source_leaf)?;
        let from_location = self.locate(source.root, &entry.location()?)?;
        let to_location =
            self.verify_child(destination.root, &destination_dir, destination_leaf)?;
        let tree = entry.kind() == EntryKind::Dir;
        if tree {
            self.move_keeps_denials(&from_location, &to_location)?;
        }
        self.move_keeps_access(&from_location, &to_location, tree)?;
        // An existing destination (possibly reached through an alias such as
        // an 8.3 name) is judged by its real location before it is replaced
        match destination_dir.entry(destination_leaf) {
            Ok(existing) => self.verify_location(destination.root, &existing.location()?)?,
            Err(FsError::Io(e)) if e.kind() == io::ErrorKind::NotFound => {}
            // A link at the destination is replaced as an entry, never followed
            Err(FsError::PathDenied(Denial::ObjectLocation)) => {}
            Err(e) => return Err(e),
        }
        entry.rename(&destination_dir, destination_leaf)
    }

    /// `fs.copy_file`: READ on the source, CREATE on the destination. Never
    /// overwrites. Returns the number of bytes copied.
    pub fn copy_file(&self, from: &Path, to: &Path) -> Result<u64, FsError> {
        // Both capabilities are checked before either path is opened
        let source = self.resolve(FsAccess::READ, from)?;
        let destination = self.resolve(FsAccess::CREATE, to)?;
        let mut input = source.root.safe().open_file(&source.rel)?;
        self.verify(source.root, &input)?;
        self.single_link(&input)?;

        let (parent, leaf) = destination.split()?;
        let dir = destination.root.safe().open_dir(&parent)?;
        self.verify_child(destination.root, &dir, leaf)?;
        let mut output = dir.create_file(leaf, Create::New)?;
        Ok(io::copy(&mut input, &mut output)?)
    }

    /// `fs.read_dir`: LIST. Denied entries, links and reparse points are
    /// omitted, never flagged.
    pub fn list_dir(&self, path: &Path) -> Result<Vec<DirEntry>, FsError> {
        let target = self.resolve(FsAccess::LIST, path)?;
        let dir = target.root.safe().open_dir(&target.rel)?;
        let here = self.locate(target.root, &safe::location(dir.as_file())?)?;
        let mut visible = dir.entries()?;
        visible.retain(|entry| !self.denied(&here.join([&Key::of(entry.name())])));
        Ok(visible)
    }

    /// `fs.exists`: METADATA. `false` only for genuine absence; a denied path
    /// is an error, never `false`.
    pub fn exists(&self, path: &Path) -> Result<bool, FsError> {
        let target = self.resolve(FsAccess::METADATA, path)?;
        match target.root.safe().probe(&target.rel) {
            Ok(file) => self.verify(target.root, &file).map(|()| true),
            Err(FsError::Io(e)) if e.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// `fs.size`: METADATA.
    pub fn size(&self, path: &Path) -> Result<u64, FsError> {
        let target = self.resolve(FsAccess::METADATA, path)?;
        let file = target.root.safe().probe(&target.rel)?;
        self.verify(target.root, &file)?;
        self.single_link(&file)?;
        Ok(file.metadata()?.len())
    }

    fn exceeds_limit(&self, contents: &[u8]) -> bool {
        u64::try_from(contents.len()).map_or(true, |len| len > self.max_file_size)
    }

    fn open_existing(&self, path: &Path) -> Result<File, FsError> {
        let target = self.resolve(FsAccess::WRITE, path)?;
        let (parent, leaf) = target.split()?;
        let dir = target.root.safe().open_dir(&parent)?;
        let file = dir.create_file(leaf, Create::Existing)?;
        // Verified before the first byte changes: truncation comes after
        self.verify(target.root, &file)?;
        self.single_link(&file)?;
        file.set_len(0)?;
        Ok(file)
    }

    fn create_new(&self, path: &Path) -> Result<File, FsError> {
        let target = self.resolve(FsAccess::CREATE, path)?;
        let (parent, leaf) = target.split()?;
        let dir = target.root.safe().open_dir(&parent)?;
        self.verify_child(target.root, &dir, leaf)?;
        dir.create_file(leaf, Create::New)
    }

    fn remove(&self, path: &Path, kind: EntryKind) -> Result<(), FsError> {
        let target = self.resolve(FsAccess::DELETE, path)?;
        let (parent, leaf) = target.split()?;
        let dir = target.root.safe().open_dir(&parent)?;
        let entry = dir.entry(leaf)?;
        let is_dir = entry.kind() == EntryKind::Dir;
        if kind == EntryKind::Dir && !is_dir {
            return Err(io::Error::from(io::ErrorKind::NotADirectory).into());
        }
        if kind != EntryKind::Dir && is_dir {
            return Err(io::Error::from(io::ErrorKind::IsADirectory).into());
        }
        self.verify_location(target.root, &entry.location()?)?;
        entry.remove()
    }

    /// Rules 1-4: parse, capability, anchor, lexical deny.
    fn resolve(&self, required: FsAccess, path: &Path) -> Result<Target<'a>, FsError> {
        let request = parse_request(path)?;
        let eligible: Vec<&'a Root> = self
            .grants
            .iter()
            .filter(|(_, access)| access.contains(required))
            .flat_map(|(scope, _)| scope.roots())
            .collect();
        if !self
            .grants
            .iter()
            .any(|(_, access)| access.contains(required))
        {
            return Err(FsError::CapabilityDenied);
        }

        let (root, rel) = match request {
            Request::Absolute { volume, names } => {
                let mut best: Option<(usize, &'a Root, RelPath)> = None;
                for &root in &eligible {
                    if let Some((specificity, rel)) = root.anchor(&volume, &names)
                        && best
                            .as_ref()
                            .is_none_or(|(current, _, _)| specificity > *current)
                    {
                        best = Some((specificity, root, rel));
                    }
                }
                let (_, root, rel) = best.ok_or(FsError::PathDenied(Denial::OutsideRoots))?;
                (root, rel)
            }
            Request::Relative(rel) => {
                let root = self.sole_root()?;
                if !eligible
                    .iter()
                    .any(|&candidate| std::ptr::eq(candidate, root))
                {
                    return Err(FsError::CapabilityDenied);
                }
                if !root.admits(&rel) {
                    return Err(FsError::PathDenied(Denial::OutsideRoots));
                }
                (root, rel)
            }
        };

        let location = root.canonical().join(rel.names().iter().map(Name::key));
        if self.denied(&location) {
            return Err(FsError::PathDenied(Denial::DenyRule));
        }
        Ok(Target { root, rel })
    }

    /// The single root a relative request anchors at.
    fn sole_root(&self) -> Result<&'a Root, FsError> {
        let mut roots = self.grants.iter().flat_map(|(scope, _)| scope.roots());
        let first = roots
            .next()
            .ok_or(FsError::PathDenied(Denial::OutsideRoots))?;
        if roots.all(|root| std::ptr::eq(root, first)) {
            Ok(first)
        } else {
            Err(FsError::InvalidPath(
                "relative paths need an absolute form when several roots are allowed",
            ))
        }
    }

    /// Deny wins across grants; any scope of the origin may veto.
    fn denied(&self, location: &Location) -> bool {
        self.grants
            .iter()
            .flat_map(|(scope, _)| scope.roots())
            .any(|root| root.denies(location))
    }

    /// Every root of every scope the origin holds.
    fn roots(&self) -> impl Iterator<Item = &'a Root> + '_ {
        self.grants.iter().flat_map(|(scope, _)| scope.roots())
    }

    /// Moving a directory from `from` to `to` must keep every deny rule over
    /// what it contains. A `deny_path` below `from` must still be denied at
    /// its new location (denial is inherited by descendants, so this is
    /// exact). A glob still live below `from` must be at least as live below
    /// `to`, compared by value across every root, so a move between roots
    /// sharing the same glob stays allowed.
    fn move_keeps_denials(&self, from: &Location, to: &Location) -> Result<(), FsError> {
        let refused = || FsError::PathDenied(Denial::DenyRule);
        for root in self.roots() {
            for suffix in root.enclosed_denies(from) {
                if !self.denied(&to.join(suffix)) {
                    return Err(refused());
                }
            }
            for (glob, residual) in root.glob_residuals(from) {
                // `Covered` means `from` itself is denied, refused earlier
                let Residual::Live(needed) = residual else {
                    continue;
                };
                let kept = self
                    .roots()
                    .flat_map(|root| root.glob_residuals(to))
                    .filter(|(other, _)| *other == glob)
                    .fold(0, |kept, (_, residual)| match residual {
                        Residual::Covered => u64::MAX,
                        Residual::Live(live) => kept | live,
                    });
                if needed & !kept != 0 {
                    return Err(refused());
                }
            }
        }
        Ok(())
    }

    /// A move may not grant the origin more over the moved object than it
    /// held at the source: no capability at `to` it lacked at `from`, and for
    /// a directory none reaching its contents that did not reach them before
    /// (a non-recursive root hides grandchildren a recursive one would show).
    fn move_keeps_access(&self, from: &Location, to: &Location, tree: bool) -> Result<(), FsError> {
        let mut gained = self.access_at(to).without(self.access_at(from));
        if tree {
            gained |= self.access_beneath(to).without(self.access_beneath(from));
        }
        if gained.is_empty() {
            Ok(())
        } else {
            Err(FsError::PathDenied(Denial::ObjectLocation))
        }
    }

    /// The capabilities of the grants whose roots reach `location`.
    fn access_at(&self, location: &Location) -> FsAccess {
        self.grants
            .iter()
            .filter(|(scope, _)| scope.roots().iter().any(|root| root.reaches(location)))
            .fold(FsAccess::NONE, |all, (_, access)| all | *access)
    }

    /// The capabilities of the grants that reach every descendant of `location`.
    fn access_beneath(&self, location: &Location) -> FsAccess {
        self.grants
            .iter()
            .filter(|(scope, _)| {
                scope
                    .roots()
                    .iter()
                    .any(|root| root.reaches_beneath(location))
            })
            .fold(FsAccess::NONE, |all, (_, access)| all | *access)
    }

    /// Refuses a regular file that has other names, unless the configuration
    /// allows hard links: its other names may lie outside every root or under
    /// a deny rule, and the location re-check only sees the name used here.
    /// Directories are exempt; their link counts count subdirectories.
    fn single_link(&self, file: &File) -> Result<(), FsError> {
        if self.allow_hard_links || !file.metadata()?.is_file() {
            return Ok(());
        }
        if safe::link_count(file)? > 1 {
            return Err(FsError::PathDenied(Denial::ObjectLocation));
        }
        Ok(())
    }

    /// Rule 5 for an opened object.
    fn verify(&self, anchor: &Root, object: &File) -> Result<(), FsError> {
        self.locate(anchor, &safe::location(object)?).map(drop)
    }

    /// Rule 5 for a new entry; its parent's real location plus its name,
    /// which is returned.
    fn verify_child(&self, anchor: &Root, dir: &Dir, leaf: &Name) -> Result<Location, FsError> {
        let parent = self.locate(anchor, &safe::location(dir.as_file())?)?;
        let child = parent.join([leaf.key()]);
        if self.denied(&child) {
            return Err(FsError::PathDenied(Denial::ObjectLocation));
        }
        Ok(child)
    }

    fn verify_location(&self, anchor: &Root, reported: &Path) -> Result<(), FsError> {
        self.locate(anchor, reported).map(drop)
    }

    /// Parses a kernel-reported location and applies rule 5 to it.
    fn locate(&self, anchor: &Root, reported: &Path) -> Result<Location, FsError> {
        let location =
            Location::parse(reported).ok_or(FsError::PathDenied(Denial::ObjectLocation))?;
        if location.strip(anchor.canonical()).is_none() || self.denied(&location) {
            return Err(FsError::PathDenied(Denial::ObjectLocation));
        }
        Ok(location)
    }
}

#[cfg(all(test, any(target_os = "linux", windows, target_os = "macos")))]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use crate::capability::test_support::{link_dir, link_file};

    fn origin() -> Origin {
        Origin::parse("app://notes").unwrap()
    }

    /// `notes/{note.txt, ok.txt, secrets/secret.txt}`, recursive allow of
    /// `notes`, `notes/secrets` denied.
    fn fixture(access: FsAccess) -> (tempfile::TempDir, PathBuf, Filesystem) {
        let tmp = tempfile::tempdir().unwrap();
        let notes = tmp.path().join("notes");
        std::fs::create_dir_all(notes.join("secrets")).unwrap();
        std::fs::write(notes.join("secrets/secret.txt"), b"top secret").unwrap();
        std::fs::write(notes.join("note.txt"), b"hello").unwrap();
        std::fs::write(notes.join("ok.txt"), b"1").unwrap();
        let mut builder = Filesystem::builder();
        let scope = builder.scope("notes", |s| {
            s.allow_directory_recursive(&notes);
            s.deny_path(notes.join("secrets"));
        });
        builder.grant(origin(), scope, access);
        (tmp, notes, builder.build().unwrap())
    }

    fn denial(result: Result<impl std::fmt::Debug, FsError>) -> Denial {
        match result {
            Err(FsError::PathDenied(denial)) => denial,
            other => panic!("expected a path denial, got {other:?}"),
        }
    }

    #[test]
    fn scope_handles_belong_to_their_builder() {
        let mut other = Filesystem::builder();
        let foreign = other.scope("other", |_| {});
        let mut builder = Filesystem::builder();
        builder.grant(origin(), foreign, FsAccess::READ);
        assert!(matches!(builder.build(), Err(FsConfigError::ForeignScope)));
    }

    #[test]
    fn the_opaque_origin_cannot_be_granted() {
        let mut builder = Filesystem::builder();
        let scope = builder.scope("empty", |_| {});
        builder.grant(Origin::OPAQUE, scope, FsAccess::READ);
        assert!(matches!(builder.build(), Err(FsConfigError::OpaqueOrigin)));
    }

    #[test]
    fn only_granted_origins_hold_authority() {
        let (_tmp, _notes, fs) = fixture(FsAccess::ALL);
        assert!(fs.authorize(&origin()).is_some());
        assert!(
            fs.authorize(&Origin::parse("app://other").unwrap())
                .is_none()
        );
        assert!(fs.authorize(&Origin::OPAQUE).is_none());
        assert!(
            fs.authorize(&Origin::from_url("file:///etc/passwd"))
                .is_none()
        );
    }

    #[test]
    fn reads_and_metadata_work_inside_the_grant() {
        let (_tmp, notes, fs) = fixture(FsAccess::READ | FsAccess::METADATA);
        let auth = fs.authorize(&origin()).unwrap();
        assert_eq!(auth.read_file(&notes.join("note.txt")).unwrap(), b"hello");
        assert_eq!(auth.size(&notes.join("note.txt")).unwrap(), 5);
        assert!(auth.exists(&notes.join("note.txt")).unwrap());
        assert!(!auth.exists(&notes.join("missing.txt")).unwrap());
    }

    #[test]
    fn paths_outside_every_root_are_denied() {
        let (_tmp, notes, fs) = fixture(FsAccess::ALL);
        let auth = fs.authorize(&origin()).unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("o.txt"), b"o").unwrap();
        assert_eq!(
            denial(auth.read_file(&outside.path().join("o.txt"))),
            Denial::OutsideRoots
        );
        assert_eq!(
            denial(auth.read_file(&notes.join("..").join("escape.txt"))),
            Denial::OutsideRoots
        );
        assert_eq!(
            denial(auth.exists(&outside.path().join("o.txt"))),
            Denial::OutsideRoots
        );
    }

    #[test]
    fn deny_rules_win_for_every_operation() {
        let (_tmp, notes, fs) = fixture(FsAccess::ALL);
        let auth = fs.authorize(&origin()).unwrap();
        let secret = notes.join("secrets/secret.txt");
        assert_eq!(denial(auth.read_file(&secret)), Denial::DenyRule);
        assert_eq!(denial(auth.exists(&secret)), Denial::DenyRule);
        assert_eq!(denial(auth.size(&secret)), Denial::DenyRule);
        assert_eq!(denial(auth.write_file(&secret, b"x")), Denial::DenyRule);
        assert_eq!(denial(auth.remove_file(&secret)), Denial::DenyRule);
        assert_eq!(
            denial(auth.list_dir(&notes.join("secrets"))),
            Denial::DenyRule
        );
        assert_eq!(std::fs::read(&secret).unwrap(), b"top secret");
    }

    #[test]
    fn links_into_a_denied_subtree_reach_nothing() {
        let (_tmp, notes, fs) = fixture(FsAccess::ALL);
        link_dir(&notes.join("secrets"), &notes.join("alias"));
        let auth = fs.authorize(&origin()).unwrap();
        let alias = notes.join("alias");

        assert_eq!(
            denial(auth.read_file(&alias.join("secret.txt"))),
            Denial::ObjectLocation
        );
        assert_eq!(denial(auth.list_dir(&alias)), Denial::ObjectLocation);
        assert!(auth.write_file(&alias.join("new.txt"), b"x").is_err());
        assert!(auth.create_dir(&alias.join("dir")).is_err());
        assert!(
            auth.copy_file(&notes.join("note.txt"), &alias.join("copy.txt"))
                .is_err()
        );
        assert!(
            auth.rename_file(&notes.join("ok.txt"), &alias.join("moved.txt"))
                .is_err()
        );
        assert!(auth.remove_file(&alias.join("secret.txt")).is_err());

        for leaked in ["new.txt", "dir", "copy.txt", "moved.txt"] {
            assert!(
                !notes.join("secrets").join(leaked).exists(),
                "{leaked} leaked"
            );
        }
        assert!(notes.join("secrets/secret.txt").exists());
        assert!(notes.join("ok.txt").exists());
    }

    #[test]
    fn links_to_allowed_places_are_not_followed_either() {
        let (_tmp, notes, fs) = fixture(FsAccess::READ | FsAccess::LIST);
        std::fs::create_dir(notes.join("docs")).unwrap();
        std::fs::write(notes.join("docs/readme.txt"), b"r").unwrap();
        link_dir(&notes.join("docs"), &notes.join("docs_link"));
        let auth = fs.authorize(&origin()).unwrap();
        assert!(auth.read_file(&notes.join("docs/readme.txt")).is_ok());
        assert_eq!(
            denial(auth.read_file(&notes.join("docs_link/readme.txt"))),
            Denial::ObjectLocation
        );
        if link_file(&notes.join("note.txt"), &notes.join("note_link.txt")) {
            assert_eq!(
                denial(auth.read_file(&notes.join("note_link.txt"))),
                Denial::ObjectLocation
            );
        }
    }

    #[test]
    fn listings_omit_denied_entries_and_links() {
        let (_tmp, notes, fs) = fixture(FsAccess::LIST);
        link_dir(&notes.join("secrets"), &notes.join("alias"));
        let auth = fs.authorize(&origin()).unwrap();
        let names: Vec<_> = auth
            .list_dir(&notes)
            .unwrap()
            .iter()
            .map(|e| e.name().to_string_lossy().into_owned())
            .collect();
        assert!(names.contains(&"note.txt".to_owned()));
        assert!(!names.contains(&"secrets".to_owned()));
        assert!(!names.contains(&"alias".to_owned()));
    }

    #[test]
    fn each_operation_requires_its_capability() {
        let (_tmp, notes, fs) = fixture(FsAccess::READ | FsAccess::LIST);
        let auth = fs.authorize(&origin()).unwrap();
        let note = notes.join("note.txt");
        assert!(matches!(
            auth.write_file(&note, b"x"),
            Err(FsError::CapabilityDenied)
        ));
        assert!(matches!(
            auth.create_dir(&notes.join("d")),
            Err(FsError::CapabilityDenied)
        ));
        assert!(matches!(
            auth.remove_file(&note),
            Err(FsError::CapabilityDenied)
        ));
        assert!(matches!(
            auth.rename_file(&note, &notes.join("m")),
            Err(FsError::CapabilityDenied)
        ));
        assert!(matches!(
            auth.copy_file(&note, &notes.join("c")),
            Err(FsError::CapabilityDenied)
        ));
        assert!(matches!(auth.exists(&note), Err(FsError::CapabilityDenied)));
        assert!(matches!(auth.size(&note), Err(FsError::CapabilityDenied)));
    }

    // Both paths' capabilities are checked before either is opened, so a
    // missing CREATE is refused without touching the source
    #[test]
    fn copy_checks_both_capabilities_before_opening_anything() {
        let (_tmp, notes, fs) = fixture(FsAccess::READ);
        let auth = fs.authorize(&origin()).unwrap();
        assert!(matches!(
            auth.copy_file(&notes.join("absent.txt"), &notes.join("c")),
            Err(FsError::CapabilityDenied)
        ));
    }

    #[test]
    fn write_needs_write_for_existing_and_create_for_new_files() {
        let (_tmp, notes, fs) = fixture(FsAccess::CREATE);
        let auth = fs.authorize(&origin()).unwrap();
        auth.write_file(&notes.join("fresh.txt"), b"c").unwrap();
        assert!(matches!(
            auth.write_file(&notes.join("note.txt"), b"x"),
            Err(FsError::CapabilityDenied)
        ));
        assert_eq!(
            std::fs::read(notes.join("note.txt")).unwrap(),
            b"hello",
            "never truncated"
        );

        let (_tmp, notes, fs) = fixture(FsAccess::WRITE);
        let auth = fs.authorize(&origin()).unwrap();
        auth.write_file(&notes.join("note.txt"), b"zz").unwrap();
        assert_eq!(std::fs::read(notes.join("note.txt")).unwrap(), b"zz");
        assert!(matches!(
            auth.write_file(&notes.join("new.txt"), b"x"),
            Err(FsError::CapabilityDenied)
        ));
        assert!(!notes.join("new.txt").exists());
    }

    #[test]
    fn files_over_the_transfer_limit_are_refused() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("five.txt"), b"hello").unwrap();
        std::fs::write(tmp.path().join("four.txt"), b"four").unwrap();
        let mut builder = Filesystem::builder();
        let scope = builder.scope("data", |s| {
            s.allow_directory(tmp.path());
        });
        builder
            .grant(origin(), scope, FsAccess::ALL)
            .max_file_size(4);
        let fs = builder.build().unwrap();
        let auth = fs.authorize(&origin()).unwrap();
        let too_large =
            |result: Result<_, FsError>| matches!(result, Err(FsError::TooLarge { limit: 4 }));

        assert_eq!(
            auth.read_file(&tmp.path().join("four.txt")).unwrap(),
            b"four"
        );
        assert!(too_large(
            auth.read_file(&tmp.path().join("five.txt")).map(drop)
        ));
        assert!(too_large(
            auth.write_file(&tmp.path().join("four.txt"), b"12345")
        ));
        assert_eq!(
            std::fs::read(tmp.path().join("four.txt")).unwrap(),
            b"four",
            "never truncated"
        );
        assert!(too_large(
            auth.write_file(&tmp.path().join("new.txt"), b"12345")
        ));
        assert!(!tmp.path().join("new.txt").exists(), "never created");
        auth.write_file(&tmp.path().join("new.txt"), b"1234")
            .unwrap();
        assert_eq!(
            auth.copy_file(&tmp.path().join("five.txt"), &tmp.path().join("copy.txt"))
                .unwrap(),
            5
        );
    }

    #[test]
    fn copy_never_overwrites_and_rename_replaces() {
        let (_tmp, notes, fs) = fixture(FsAccess::ALL);
        let auth = fs.authorize(&origin()).unwrap();
        assert_eq!(
            auth.copy_file(&notes.join("note.txt"), &notes.join("copy.txt"))
                .unwrap(),
            5
        );
        let again = auth.copy_file(&notes.join("ok.txt"), &notes.join("copy.txt"));
        assert!(
            matches!(again, Err(FsError::Io(ref e)) if e.kind() == io::ErrorKind::AlreadyExists)
        );
        assert_eq!(std::fs::read(notes.join("copy.txt")).unwrap(), b"hello");

        auth.rename_file(&notes.join("ok.txt"), &notes.join("copy.txt"))
            .unwrap();
        assert!(!notes.join("ok.txt").exists());
        assert_eq!(std::fs::read(notes.join("copy.txt")).unwrap(), b"1");
    }

    #[test]
    fn directory_lifecycle_and_kind_checks() {
        let (_tmp, notes, fs) = fixture(FsAccess::ALL);
        let auth = fs.authorize(&origin()).unwrap();
        let dir = notes.join("scratch");
        auth.create_dir(&dir).unwrap();
        auth.write_file(&dir.join("one.txt"), b"1").unwrap();
        assert!(
            matches!(auth.remove_dir(&dir), Err(FsError::Io(ref e)) if e.kind() == io::ErrorKind::DirectoryNotEmpty)
        );
        assert!(matches!(auth.remove_file(&dir), Err(FsError::Io(_))));
        assert!(matches!(
            auth.remove_dir(&dir.join("one.txt")),
            Err(FsError::Io(_))
        ));
        auth.remove_file(&dir.join("one.txt")).unwrap();
        auth.remove_dir(&dir).unwrap();
        assert!(!dir.exists());
        assert!(
            matches!(auth.create_dir(&notes.join("a/b")), Err(FsError::Io(_))),
            "single level only"
        );
        assert!(
            matches!(auth.remove_dir(&notes), Err(FsError::InvalidPath(_))),
            "roots stay"
        );
    }

    #[test]
    fn rename_authorizes_source_and_destination_independently() {
        let (tmp, notes, fs) = fixture(FsAccess::RENAME);
        let auth = fs.authorize(&origin()).unwrap();
        std::fs::create_dir(tmp.path().join("outside")).unwrap();
        let out = auth.rename_file(&notes.join("ok.txt"), &tmp.path().join("outside/o.txt"));
        assert_eq!(denial(out), Denial::OutsideRoots);
        assert!(notes.join("ok.txt").exists());
    }

    #[test]
    fn relative_paths_need_exactly_one_root() {
        let (_tmp, notes, fs) = fixture(FsAccess::READ);
        let auth = fs.authorize(&origin()).unwrap();
        assert_eq!(auth.read_file(Path::new("note.txt")).unwrap(), b"hello");
        assert_eq!(
            auth.read_file(Path::new("a/../note.txt")).unwrap(),
            b"hello"
        );
        assert_eq!(
            denial(auth.read_file(Path::new("../x"))),
            Denial::OutsideRoots
        );
        assert_eq!(
            denial(auth.read_file(Path::new("secrets/secret.txt"))),
            Denial::DenyRule
        );

        let other = tempfile::tempdir().unwrap();
        let mut builder = Filesystem::builder();
        let a = builder.scope("a", |s| {
            s.allow_directory_recursive(&notes);
        });
        let b = builder.scope("b", |s| {
            s.allow_directory_recursive(other.path());
        });
        builder
            .grant(origin(), a, FsAccess::READ)
            .grant(origin(), b, FsAccess::READ);
        let two = builder.build().unwrap();
        let auth = two.authorize(&origin()).unwrap();
        assert!(matches!(
            auth.read_file(Path::new("note.txt")),
            Err(FsError::InvalidPath(_))
        ));
        assert!(auth.read_file(&notes.join("note.txt")).is_ok());
    }

    #[test]
    fn grants_combine_but_deny_wins_across_them() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        std::fs::create_dir_all(data.join("private")).unwrap();
        std::fs::write(data.join("private/p.txt"), b"p").unwrap();
        std::fs::write(data.join("a.txt"), b"a").unwrap();
        let mut builder = Filesystem::builder();
        let read = builder.scope("read", |s| {
            s.allow_directory_recursive(&data);
            s.deny_path(data.join("private"));
        });
        let write = builder.scope("write", |s| {
            s.allow_directory_recursive(data.join("private"));
        });
        builder.grant(origin(), read, FsAccess::READ).grant(
            origin(),
            write,
            FsAccess::WRITE | FsAccess::READ,
        );
        let fs = builder.build().unwrap();
        let auth = fs.authorize(&origin()).unwrap();
        assert!(auth.read_file(&data.join("a.txt")).is_ok());
        assert_eq!(
            denial(auth.read_file(&data.join("private/p.txt"))),
            Denial::DenyRule
        );
        assert_eq!(
            denial(auth.write_file(&data.join("private/p.txt"), b"x")),
            Denial::DenyRule
        );
        // WRITE is granted, but only over `private/`
        assert_eq!(
            denial(auth.write_file(&data.join("a.txt"), b"x")),
            Denial::OutsideRoots
        );
    }

    #[test]
    fn renaming_a_directory_that_encloses_a_deny_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        std::fs::create_dir_all(root.join("vault/inner")).unwrap();
        std::fs::write(root.join("vault/inner/secret.txt"), b"top secret").unwrap();
        let mut builder = Filesystem::builder();
        let scope = builder.scope("root", |s| {
            s.allow_directory_recursive(&root);
            s.deny_path(root.join("vault/inner"));
        });
        builder.grant(origin(), scope, FsAccess::ALL);
        let fs = builder.build().unwrap();
        let auth = fs.authorize(&origin()).unwrap();

        // `vault/` is allowed and renameable, but it encloses the denied
        // `vault/inner/`, whose deny rule is anchored by a fixed prefix.
        let moved = root.join("relocated");
        assert_eq!(
            denial(auth.rename_file(&root.join("vault"), &moved)),
            Denial::DenyRule
        );
        assert!(
            !moved.exists(),
            "the enclosing directory must not have moved"
        );
        assert!(root.join("vault/inner/secret.txt").exists());
        assert_eq!(
            denial(auth.read_file(&root.join("vault/inner/secret.txt"))),
            Denial::DenyRule
        );

        // A directory that encloses no deny still renames normally.
        std::fs::create_dir(root.join("plain")).unwrap();
        auth.rename_file(&root.join("plain"), &root.join("plain2"))
            .unwrap();
        assert!(root.join("plain2").exists());
    }

    #[test]
    fn renaming_a_directory_cannot_escape_a_glob_deny() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        std::fs::create_dir_all(root.join("vault")).unwrap();
        std::fs::create_dir_all(root.join("p/secrets")).unwrap();
        std::fs::write(root.join("vault/a.key"), b"k").unwrap();
        std::fs::write(root.join("p/secrets/tls.pem"), b"p").unwrap();
        let mut builder = Filesystem::builder();
        let scope = builder.scope("root", |s| {
            s.allow_directory_recursive(&root);
            s.deny_glob("vault/*.key");
            s.deny_glob("**/secrets/*.pem");
        });
        builder.grant(origin(), scope, FsAccess::ALL);
        let fs = builder.build().unwrap();
        let auth = fs.authorize(&origin()).unwrap();

        // Each rule depends on a directory's name; renaming it would free
        // the files beneath
        assert_eq!(
            denial(auth.rename_file(&root.join("vault"), &root.join("x"))),
            Denial::DenyRule
        );
        assert_eq!(
            denial(auth.rename_file(&root.join("p/secrets"), &root.join("p/open"))),
            Denial::DenyRule
        );
        // Moving the enclosing directory keeps `**/secrets/*.pem` in force
        auth.rename_file(&root.join("p"), &root.join("q")).unwrap();
        assert_eq!(
            denial(auth.read_file(&root.join("q/secrets/tls.pem"))),
            Denial::DenyRule
        );
    }

    #[test]
    fn renames_cannot_gain_access() {
        let tmp = tempfile::tempdir().unwrap();
        let (x, y) = (tmp.path().join("x"), tmp.path().join("y"));
        std::fs::create_dir_all(x.join("d/deep")).unwrap();
        std::fs::create_dir(&y).unwrap();
        std::fs::write(x.join("f.txt"), b"f").unwrap();
        std::fs::write(x.join("d/deep/g.txt"), b"g").unwrap();
        let mut builder = Filesystem::builder();
        let sx = builder.scope("x", |s| {
            s.allow_directory(&x);
        });
        let sy = builder.scope("y", |s| {
            s.allow_directory_recursive(&y);
        });
        builder.grant(origin(), sx, FsAccess::RENAME).grant(
            origin(),
            sy,
            FsAccess::RENAME | FsAccess::READ,
        );
        let fs = builder.build().unwrap();
        let auth = fs.authorize(&origin()).unwrap();

        // READ over `y` is not READ over what `x` holds
        assert_eq!(
            denial(auth.rename_file(&x.join("f.txt"), &y.join("f.txt"))),
            Denial::ObjectLocation
        );
        // Nor may a directory carry `x`'s hidden grandchildren into reach
        assert_eq!(
            denial(auth.rename_file(&x.join("d"), &y.join("d"))),
            Denial::ObjectLocation
        );
        assert!(x.join("f.txt").exists() && x.join("d/deep/g.txt").exists());
        // Losing access is fine
        std::fs::write(y.join("h.txt"), b"h").unwrap();
        auth.rename_file(&y.join("h.txt"), &x.join("h.txt"))
            .unwrap();
    }

    #[test]
    fn multiply_linked_files_are_refused() {
        let (tmp, notes, fs) = fixture(FsAccess::ALL);
        let outside = tmp.path().join("outside.txt");
        std::fs::write(&outside, b"outside").unwrap();
        std::fs::hard_link(&outside, notes.join("linked.txt")).unwrap();
        let auth = fs.authorize(&origin()).unwrap();
        let linked = notes.join("linked.txt");
        assert_eq!(denial(auth.read_file(&linked)), Denial::ObjectLocation);
        assert_eq!(denial(auth.size(&linked)), Denial::ObjectLocation);
        assert_eq!(
            denial(auth.write_file(&linked, b"x")),
            Denial::ObjectLocation
        );
        assert_eq!(
            denial(auth.copy_file(&linked, &notes.join("c.txt"))),
            Denial::ObjectLocation
        );
        assert_eq!(
            std::fs::read(&outside).unwrap(),
            b"outside",
            "never truncated"
        );
        // The name itself is an ordinary entry
        assert!(auth.exists(&linked).unwrap());
        auth.remove_file(&linked).unwrap();

        let mut builder = Filesystem::builder();
        let scope = builder.scope("notes", |s| {
            s.allow_directory_recursive(&notes);
        });
        builder
            .grant(origin(), scope, FsAccess::ALL)
            .allow_hard_links(true);
        let allowing = builder.build().unwrap();
        std::fs::hard_link(&outside, notes.join("linked.txt")).unwrap();
        let auth = allowing.authorize(&origin()).unwrap();
        assert_eq!(auth.read_file(&linked).unwrap(), b"outside");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_case_and_normalization_variants_hit_deny_rules() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        std::fs::create_dir_all(root.join("Secrets")).unwrap();
        std::fs::write(root.join("Secrets/key.txt"), b"k").unwrap();
        std::fs::write(root.join("caf\u{e9}.txt"), b"c").unwrap();
        let mut builder = Filesystem::builder();
        let scope = builder.scope("root", |s| {
            s.allow_directory_recursive(&root);
            s.deny_path(root.join("Secrets"));
            s.deny_glob("**/caf\u{e9}.txt");
        });
        builder.grant(origin(), scope, FsAccess::ALL);
        let fs = builder.build().unwrap();
        let auth = fs.authorize(&origin()).unwrap();
        // The default volumes fold case and normalization; so do the keys,
        // whatever the kernel reports for the opened object
        for variant in [
            root.join("SECRETS/key.txt"),
            root.join("secrets/KEY.TXT"),
            root.join("cafe\u{301}.txt"),
            root.join("CAF\u{c9}.TXT"),
            root.join("sec\u{200c}rets/key.txt"),
        ] {
            assert_eq!(
                denial(auth.read_file(&variant)),
                Denial::DenyRule,
                "{}",
                variant.display()
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_case_variants_and_streams_are_refused() {
        let (_tmp, notes, fs) = fixture(FsAccess::ALL);
        let auth = fs.authorize(&origin()).unwrap();
        assert_eq!(
            denial(auth.read_file(&notes.join("SECRETS").join("secret.txt"))),
            Denial::DenyRule
        );
        assert_eq!(
            denial(auth.read_file(&notes.join("Secrets").join("SECRET.TXT"))),
            Denial::DenyRule
        );
        assert!(matches!(
            auth.read_file(&notes.join("note.txt:stream")),
            Err(FsError::InvalidPath(_))
        ));
        assert!(matches!(
            auth.read_file(&notes.join("note.txt::$DATA")),
            Err(FsError::InvalidPath(_))
        ));
        assert!(matches!(
            auth.write_file(&notes.join("trailing."), b"x"),
            Err(FsError::InvalidPath(_))
        ));
        assert!(matches!(
            auth.write_file(&notes.join("CON"), b"x"),
            Err(FsError::InvalidPath(_))
        ));
    }

    #[cfg(windows)]
    #[test]
    fn windows_short_names_cannot_alias_denied_files() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        std::fs::create_dir(&data).unwrap();
        let long = data.join("settings-file.json");
        std::fs::write(&long, b"{}").unwrap();
        let Some(short) = short_name(&long).filter(|s| s != long.file_name().unwrap()) else {
            eprintln!("skipped: 8.3 short names are disabled on this volume");
            return;
        };
        let mut builder = Filesystem::builder();
        let scope = builder.scope("data", |s| {
            s.allow_directory_recursive(&data);
            s.deny_glob("**/*.json");
        });
        builder.grant(origin(), scope, FsAccess::ALL);
        let fs = builder.build().unwrap();
        let auth = fs.authorize(&origin()).unwrap();
        let alias = data.join(&short);
        let absent = data.join("QZQZQZ~7.JSO");
        let invalid = |r: Result<_, FsError>| matches!(r, Err(FsError::InvalidPath(_)));
        // Short-name shapes are refused before the filesystem is consulted,
        // so the answer cannot depend on whether a denied file stands behind
        for target in [&alias, &absent] {
            assert!(invalid(auth.read_file(target).map(drop)));
            assert!(invalid(auth.exists(target).map(drop)));
            assert!(invalid(auth.write_file(target, b"x")));
            assert!(invalid(auth.remove_file(target)));
            assert!(invalid(auth.rename_file(target, &data.join("free.txt"))));
        }
        std::fs::write(data.join("plain.txt"), b"p").unwrap();
        assert!(invalid(auth.rename_file(&data.join("plain.txt"), &alias)));
        assert_eq!(std::fs::read(&long).unwrap(), b"{}");
        assert!(!absent.exists());
    }

    #[cfg(windows)]
    fn short_name(path: &Path) -> Option<std::ffi::OsString> {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;

        let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        let mut buf = vec![0u16; 1024];
        // SAFETY: `wide` is NUL-terminated and `buf` is writable for 1024 units
        let len = unsafe { GetShortPathNameW(wide.as_ptr(), buf.as_mut_ptr(), 1024) } as usize;
        let short = PathBuf::from(std::ffi::OsString::from_wide(buf.get(..len)?));
        (len > 0)
            .then(|| short.file_name().map(ToOwned::to_owned))
            .flatten()
    }
}

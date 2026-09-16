//! Platform-safe opening primitives for the filesystem capability layer.
//!
//! This module confines object access to an authorized root and prevents
//! links, reparse points and path races from escaping that boundary.
//!
//! Every backend implements the following contract:
//!
//! - Multi-component lookups are resolved by the kernel beneath the root
//!   handle, never by re-walking a canonicalized path.
//! - Links and reparse points below the root are never traversed. Lookups
//!   through them return [`Denial::ObjectLocation`](crate::capability::error::Denial);
//!   listings omit them, and link entries cannot be opened, removed or renamed.
//! - Mutations operate from a directory handle and one validated [`Name`],
//!   without re-resolving the authorized path.
//! - Objects expose their kernel-reported location, which the authorization
//!   layer re-checks against deny rules before use.
//!
//! This module determines how objects are opened; origins, grants and deny
//! rules are enforced by the authorization layer.
//!
//! Backends are provided for Linux (`openat2`, with `RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS`)
//! and Windows (`NtCreateFile` relative to a directory handle with `OBJ_DONT_REPARSE`).
//!
//! Other platforms fail closed.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

use crate::capability::error::FsError;
use crate::capability::path::{Name, RelPath};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as sys;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as sys;

#[cfg(not(any(target_os = "linux", windows)))]
mod unsupported;
#[cfg(not(any(target_os = "linux", windows)))]
use unsupported as sys;

/// How [`Dir::create_file`] treats its leaf.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Create {
    /// Create a new file; fails if the name exists.
    New,
    /// Open an existing regular file for writing, without truncating it.
    Existing,
}

/// The kind of a directory entry.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Dir,
    /// A device, FIFO or socket.
    Other,
}

/// One entry of a listed directory. Links and reparse points are never listed.
#[derive(Clone, Debug)]
pub struct DirEntry {
    name: OsString,
    kind: EntryKind,
}

impl DirEntry {
    pub(crate) fn new(name: OsString, kind: EntryKind) -> DirEntry {
        DirEntry { name, kind }
    }

    pub fn name(&self) -> &OsStr {
        &self.name
    }

    pub fn kind(&self) -> EntryKind {
        self.kind
    }
}

/// A directory handle whose descendants are the only reachable objects.
pub(crate) struct SafeRoot {
    handle: File,
    canonical: PathBuf,
}

impl SafeRoot {
    /// Opens `path` (trusted configuration; links in it are resolved) as a
    /// root handle.
    pub(crate) fn open(path: &Path) -> io::Result<SafeRoot> {
        let handle = sys::open_root(path)?;
        let canonical = sys::location(&handle)?;
        Ok(SafeRoot { handle, canonical })
    }

    /// The kernel-reported location of the root, in the same form as
    /// [`location`] reports objects beneath it.
    pub(crate) fn canonical(&self) -> &Path {
        &self.canonical
    }

    /// Opens the regular file `rel` for reading.
    pub(crate) fn open_file(&self, rel: &RelPath) -> Result<File, FsError> {
        sys::open_file(&self.handle, rel)
    }

    /// Opens `rel` for metadata only.
    pub(crate) fn probe(&self, rel: &RelPath) -> Result<File, FsError> {
        sys::probe(&self.handle, rel)
    }

    /// Opens the directory `rel` (the root itself when empty).
    pub(crate) fn open_dir(&self, rel: &RelPath) -> Result<Dir, FsError> {
        sys::open_dir(&self.handle, rel).map(Dir)
    }
}

/// An open directory beneath a root.
pub(crate) struct Dir(File);

impl Dir {
    pub(crate) fn as_file(&self) -> &File {
        &self.0
    }

    pub(crate) fn entries(&self) -> io::Result<Vec<DirEntry>> {
        sys::entries(&self.0)
    }

    pub(crate) fn create_file(&self, leaf: &Name, mode: Create) -> Result<File, FsError> {
        sys::create_file(&self.0, leaf, mode)
    }

    pub(crate) fn create_dir(&self, leaf: &Name) -> Result<(), FsError> {
        sys::create_dir(&self.0, leaf)
    }

    /// Opens the existing entry `leaf` for removal or renaming.
    pub(crate) fn entry(&self, leaf: &Name) -> Result<Entry<'_>, FsError> {
        let (inner, kind) = sys::open_entry(&self.0, leaf)?;
        Ok(Entry {
            dir: self,
            inner,
            kind,
        })
    }
}

/// An existing, non-link entry of a [`Dir`], held for one mutation.
pub(crate) struct Entry<'d> {
    dir: &'d Dir,
    inner: sys::Entry,
    kind: EntryKind,
}

impl Entry<'_> {
    pub(crate) fn kind(&self) -> EntryKind {
        self.kind
    }

    /// The kernel-reported location of the entry.
    pub(crate) fn location(&self) -> io::Result<PathBuf> {
        sys::entry_location(&self.dir.0, &self.inner)
    }

    pub(crate) fn remove(self) -> Result<(), FsError> {
        sys::remove(&self.dir.0, self.inner, self.kind)
    }

    /// Moves the entry to `leaf` in `to`, replacing an existing entry there.
    pub(crate) fn rename(self, to: &Dir, leaf: &Name) -> Result<(), FsError> {
        sys::rename(&self.dir.0, self.inner, &to.0, leaf)
    }
}

/// The kernel-reported location of an opened object.
pub(crate) fn location(file: &File) -> io::Result<PathBuf> {
    sys::location(file)
}

/// How many names (hard links) the opened object has.
pub(crate) fn link_count(file: &File) -> io::Result<u64> {
    sys::link_count(file)
}

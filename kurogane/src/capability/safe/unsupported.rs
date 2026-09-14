//! Backend for platforms without a safe-open implementation (macOS).
//!
//! Every operation fails closed. The macOS candidate is `openat` with
//! `O_NOFOLLOW_ANY` (macOS 11+) which maps directly onto the "never traverse
//! a link" contract.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

use super::{Create, DirEntry, EntryKind};
use crate::capability::error::FsError;
use crate::capability::path::{Name, RelPath};

/// Cannot exist on this platform: no entry can be opened.
pub(crate) enum Entry {}

fn unsupported() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "native filesystem capabilities are not implemented on this platform",
    )
}

pub(super) fn open_root(_path: &Path) -> io::Result<File> {
    Err(unsupported())
}

pub(super) fn location(_file: &File) -> io::Result<PathBuf> {
    Err(unsupported())
}

pub(super) fn open_file(_root: &File, _rel: &RelPath) -> Result<File, FsError> {
    Err(unsupported().into())
}

pub(super) fn probe(_root: &File, _rel: &RelPath) -> Result<File, FsError> {
    Err(unsupported().into())
}

pub(super) fn open_dir(_root: &File, _rel: &RelPath) -> Result<File, FsError> {
    Err(unsupported().into())
}

pub(super) fn entries(_dir: &File) -> io::Result<Vec<DirEntry>> {
    Err(unsupported())
}

pub(super) fn create_file(_dir: &File, _leaf: &Name, _mode: Create) -> Result<File, FsError> {
    Err(unsupported().into())
}

pub(super) fn create_dir(_dir: &File, _leaf: &Name) -> Result<(), FsError> {
    Err(unsupported().into())
}

pub(super) fn open_entry(_dir: &File, _leaf: &Name) -> Result<(Entry, EntryKind), FsError> {
    Err(unsupported().into())
}

pub(super) fn entry_location(_dir: &File, entry: &Entry) -> io::Result<PathBuf> {
    match *entry {}
}

pub(super) fn remove(_dir: &File, entry: Entry, _kind: EntryKind) -> Result<(), FsError> {
    match entry {}
}

pub(super) fn rename(_dir: &File, entry: Entry, _to: &File, _leaf: &Name) -> Result<(), FsError> {
    match entry {}
}

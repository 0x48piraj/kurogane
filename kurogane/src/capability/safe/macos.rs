//! macOS backend for filesystem access confined beneath a root fd.
//!
//! macOS has no `openat2`, so resolution opens the validated relative path
//! beneath the root fd with `O_NOFOLLOW_ANY` (macOS 11+), which refuses a
//! symlink met at *any* component, not just the final one. Plain
//! `O_NOFOLLOW` only covers the final component — an interior directory
//! symlink opened with it is still followed, straight out of the root — so
//! `O_NOFOLLOW_ANY` is the primitive that actually enforces "links are never
//! traversed". Because no symlink is followed and the validated names contain
//! no `..` and no separator, resolution from the root fd cannot leave the
//! root.
//!
//! A symlink met anywhere in the path (final component included) fails with
//! `ELOOP`, which is a confinement verdict (`Denial::ObjectLocation`), not an
//! I/O error. Object locations come from
//! `fcntl(F_GETPATH)`, which returns the canonical, case- and
//! normalization-preserving on-disk path the authorization layer re-checks
//! against the deny rules — the macOS analogue of Linux `/proc/self/fd` and
//! the Windows `GetFinalPathNameByHandleW` re-check, and what defeats
//! case-insensitive aliases on the default APFS/HFS+ volumes.
//!
//! A volume mounted inside a root is a link too. macOS resolution cannot be
//! told to stop at mount points, so every opened object, every leaf a
//! mutation touches and every listed directory must share the root's device;
//! anything else is refused (`Denial::ObjectLocation`) or omitted.
//!
//! Reads and existing-file writes require a regular file and open with
//! `O_NONBLOCK`, so a FIFO cannot block the worker at `open`. Leaf mutations
//! use `mkdirat` / `unlinkat` / `renameat` on the parent fd, so the object
//! that was authorized is the object that is modified.
//!
//! `O_NOFOLLOW_ANY` needs macOS 11 (Darwin 20); older kernels would ignore
//! it, so roots fail to open there.

use std::ffi::{CStr, CString, OsStr};
use std::fs::{File, OpenOptions};
use std::io;
use std::mem::MaybeUninit;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use libc::{c_int, mode_t};

use super::{Create, DirEntry, EntryKind};
use crate::capability::error::{Denial, FsError};
use crate::capability::path::{Name, RelPath};

/// `O_NOFOLLOW_ANY` (`<sys/fcntl.h>`, macOS 11+): fail if *any* component of
/// the path is a symlink, not just the final one. Plain `O_NOFOLLOW` only
/// covers the final component, so an interior directory symlink would be
/// followed and could leave the root. `libc` does not always export this, so
/// it is defined here.
const O_NOFOLLOW_ANY: c_int = 0x2000_0000;

/// An entry held for removal or renaming: its name within the parent fd.
pub(crate) struct Entry {
    leaf: Name,
}

pub(super) fn open_root(path: &Path) -> io::Result<File> {
    if !kernel_has_nofollow_any() {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "the filesystem capability needs macOS 11 or later",
        ));
    }
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY)
        .open(path)
}

/// Whether the kernel is Darwin 20 (macOS 11) or later, which knows
/// `O_NOFOLLOW_ANY`.
fn kernel_has_nofollow_any() -> bool {
    let mut uts = MaybeUninit::<libc::utsname>::zeroed();
    // SAFETY: `uts` is writable storage for one `struct utsname`
    if unsafe { libc::uname(uts.as_mut_ptr()) } != 0 {
        return false;
    }
    // SAFETY: uname succeeded, so `release` holds a NUL-terminated string
    let release = unsafe { CStr::from_ptr(uts.assume_init_ref().release.as_ptr()) };
    release
        .to_str()
        .ok()
        .and_then(|r| r.split('.').next())
        .and_then(|major| major.parse::<u32>().ok())
        .is_some_and(|major| major >= 20)
}

/// The device `fd` lives on.
fn device(fd: RawFd) -> io::Result<libc::dev_t> {
    let mut st = MaybeUninit::<libc::stat>::uninit();
    // SAFETY: `st` is writable storage for one stat and `fd` is open
    if unsafe { libc::fstat(fd, st.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fstat succeeded, so it initialized `st`
    Ok(unsafe { st.assume_init() }.st_dev)
}

/// Refuses `fd` unless it lies on the same volume as `anchor`: a different
/// device is a volume mounted inside the root.
fn same_volume(anchor: RawFd, fd: RawFd) -> Result<(), FsError> {
    if device(anchor)? == device(fd)? {
        Ok(())
    } else {
        Err(FsError::PathDenied(Denial::ObjectLocation))
    }
}

pub(super) fn location(file: &File) -> io::Result<PathBuf> {
    // F_GETPATH writes the path into a caller buffer of at least PATH_MAX.
    let mut buf = [0u8; libc::PATH_MAX as usize];
    // SAFETY: `buf` is writable for PATH_MAX bytes and the fd is open for the
    // duration of the call
    let rc = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETPATH, buf.as_mut_ptr()) };
    if rc < 0 {
        return Err(io::Error::last_os_error());
    }
    let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    Ok(PathBuf::from(OsStr::from_bytes(&buf[..len])))
}

pub(super) fn link_count(file: &File) -> io::Result<u64> {
    use std::os::unix::fs::MetadataExt;
    Ok(file.metadata()?.nlink())
}

pub(super) fn open_file(root: &File, rel: &RelPath) -> Result<File, FsError> {
    let fd = lookup(root, rel, libc::O_RDONLY | libc::O_NONBLOCK)?;
    regular(File::from(fd))
}

pub(super) fn probe(root: &File, rel: &RelPath) -> Result<File, FsError> {
    // O_NOFOLLOW_ANY (added by `lookup`) turns a symlink at any component,
    // the final one included, into ELOOP, i.e. a denial; O_NONBLOCK keeps a
    // FIFO from blocking.
    let fd = lookup(root, rel, libc::O_RDONLY | libc::O_NONBLOCK)?;
    Ok(File::from(fd))
}

pub(super) fn open_dir(root: &File, rel: &RelPath) -> Result<File, FsError> {
    let fd = lookup(root, rel, libc::O_RDONLY | libc::O_DIRECTORY)?;
    Ok(File::from(fd))
}

pub(super) fn entries(dir: &File) -> io::Result<Vec<DirEntry>> {
    let mut out = Vec::new();
    // Entries on another device are volumes mounted here
    let here = device(dir.as_raw_fd())?;
    // fdopendir consumes the fd it is given (closedir closes it), so hand it a
    // dup and keep the caller's `dir` intact.
    // SAFETY: `dir` is a live directory fd; dup returns a new owned fd or -1
    let dup = unsafe { libc::dup(dir.as_raw_fd()) };
    if dup < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `dup` is a fresh fd we own
    let owned = unsafe { OwnedFd::from_raw_fd(dup) };
    // SAFETY: `owned` is a directory fd; fdopendir takes ownership on success
    let stream = unsafe { libc::fdopendir(owned.as_raw_fd()) };
    if stream.is_null() {
        return Err(io::Error::last_os_error());
    }
    std::mem::forget(owned); // ownership moved into the DIR*, freed by closedir
    loop {
        // readdir sets errno to 0 then returns null at end; distinguish error
        // from end by checking errno.
        errno_set(0);
        // SAFETY: `stream` is a live DIR* from fdopendir
        let entry = unsafe { libc::readdir(stream) };
        if entry.is_null() {
            let err = io::Error::last_os_error();
            // SAFETY: `stream` is live and closed exactly once here
            unsafe { libc::closedir(stream) };
            return if errno_is_zero() { Ok(out) } else { Err(err) };
        }
        // SAFETY: readdir returned a live dirent valid until the next call
        let (name, d_type) = unsafe {
            let namlen = (*entry).d_namlen as usize;
            let name_ptr = (*entry).d_name.as_ptr().cast::<u8>();
            (
                std::slice::from_raw_parts(name_ptr, namlen).to_vec(),
                (*entry).d_type,
            )
        };
        if name == b"." || name == b".." {
            continue;
        }
        let kind = match d_type {
            libc::DT_REG => EntryKind::File,
            libc::DT_LNK => continue,
            // A directory may be a mount point; its stat reports the
            // mounted volume's device
            libc::DT_DIR | libc::DT_UNKNOWN => match stat_entry(dir.as_raw_fd(), &c_bytes(&name)) {
                Ok((dev, Some(kind))) if dev == here => kind,
                _ => continue,
            },
            _ => EntryKind::Other,
        };
        out.push(DirEntry::new(OsStr::from_bytes(&name).to_owned(), kind));
    }
}

pub(super) fn create_file(dir: &File, leaf: &Name, mode: Create) -> Result<File, FsError> {
    let name = c_name(leaf);
    let (flags, perm) = match mode {
        Create::New => (libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL, 0o644),
        Create::Existing => (libc::O_WRONLY | libc::O_NONBLOCK, 0),
    };
    // Never with O_NOFOLLOW, which XNU rejects alongside it (see `lookup`)
    let fd = openat(dir.as_raw_fd(), &name, flags | O_NOFOLLOW_ANY, perm)?;
    same_volume(dir.as_raw_fd(), fd.as_raw_fd())?;
    match mode {
        Create::New => Ok(File::from(fd)),
        Create::Existing => regular(File::from(fd)),
    }
}

pub(super) fn create_dir(dir: &File, leaf: &Name) -> Result<(), FsError> {
    let name = c_name(leaf);
    // SAFETY: `name` is NUL-terminated and outlives the call
    let rc = unsafe { libc::mkdirat(dir.as_raw_fd(), name.as_ptr(), 0o755) };
    check(rc)
}

pub(super) fn open_entry(dir: &File, leaf: &Name) -> Result<(Entry, EntryKind), FsError> {
    match stat_entry(dir.as_raw_fd(), &c_name(leaf))? {
        (dev, Some(kind)) if dev == device(dir.as_raw_fd())? => {
            Ok((Entry { leaf: leaf.clone() }, kind))
        }
        // A link, or a volume mounted at the leaf
        _ => Err(FsError::PathDenied(Denial::ObjectLocation)),
    }
}

pub(super) fn entry_location(dir: &File, entry: &Entry) -> io::Result<PathBuf> {
    Ok(location(dir)?.join(entry.leaf.as_os_str()))
}

pub(super) fn remove(dir: &File, entry: Entry, kind: EntryKind) -> Result<(), FsError> {
    let name = c_name(&entry.leaf);
    let flags = if kind == EntryKind::Dir {
        libc::AT_REMOVEDIR
    } else {
        0
    };
    // SAFETY: `name` is NUL-terminated and outlives the call
    let rc = unsafe { libc::unlinkat(dir.as_raw_fd(), name.as_ptr(), flags) };
    check(rc)
}

pub(super) fn rename(dir: &File, entry: Entry, to: &File, leaf: &Name) -> Result<(), FsError> {
    let from = c_name(&entry.leaf);
    let to_name = c_name(leaf);
    // SAFETY: both names are NUL-terminated and outlive the call
    let rc = unsafe {
        libc::renameat(
            dir.as_raw_fd(),
            from.as_ptr(),
            to.as_raw_fd(),
            to_name.as_ptr(),
        )
    };
    check(rc)
}

/// Opens `rel` beneath the root fd in one syscall. `O_NOFOLLOW_ANY` refuses a
/// symlink met at *any* component (POSIX `O_NOFOLLOW` covers only the final
/// one, so an interior directory symlink would otherwise be followed straight
/// out of the root). With no symlink traversed and no `..` in the validated
/// names, resolution from the root fd cannot leave the root. A symlink is
/// `ELOOP`, a confinement verdict.
///
/// `O_NOFOLLOW_ANY` covers the final component too, and must be passed
/// alone: XNU's `vn_open_auth` answers `EINVAL` when it is combined with
/// `O_NOFOLLOW`.
fn lookup(root: &File, rel: &RelPath, flags: c_int) -> Result<OwnedFd, FsError> {
    let fd = openat(root.as_raw_fd(), &c_path(rel), flags | O_NOFOLLOW_ANY, 0)?;
    same_volume(root.as_raw_fd(), fd.as_raw_fd())?;
    Ok(fd)
}

fn openat(dirfd: RawFd, name: &CStr, flags: c_int, mode: mode_t) -> Result<OwnedFd, FsError> {
    // SAFETY: `name` is NUL-terminated and outlives the call
    let rc = unsafe { libc::openat(dirfd, name.as_ptr(), flags | libc::O_CLOEXEC, mode as c_int) };
    if rc < 0 {
        return Err(lookup_error(io::Error::last_os_error()));
    }
    // SAFETY: a non-negative return is a new descriptor we now own
    Ok(unsafe { OwnedFd::from_raw_fd(rc) })
}

/// `ELOOP` means a symlink was met (`O_NOFOLLOW_ANY` / `O_NOFOLLOW`): a
/// confinement verdict, not an I/O error. (Mount points are caught by the
/// device checks; macOS resolution itself never reports them.)
fn lookup_error(err: io::Error) -> FsError {
    match err.raw_os_error() {
        Some(libc::ELOOP) => FsError::PathDenied(Denial::ObjectLocation),
        _ => FsError::Io(err),
    }
}

/// Requires a regular file, then clears the `O_NONBLOCK` that only kept `open`
/// from waiting on a FIFO.
fn regular(file: File) -> Result<File, FsError> {
    let file_type = file.metadata()?.file_type();
    if file_type.is_dir() {
        return Err(io::Error::from(io::ErrorKind::IsADirectory).into());
    }
    if !file_type.is_file() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a regular file").into());
    }
    // SAFETY: fcntl on a descriptor we own, with integer arguments only
    let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
    check(flags)?;
    // SAFETY: as above
    let rc = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFL, flags & !libc::O_NONBLOCK) };
    check(rc)?;
    Ok(file)
}

/// The device and kind of `name` in `dirfd` without following it; the kind
/// is `None` for a link. A mount point reports the mounted volume's device.
fn stat_entry(dirfd: RawFd, name: &CStr) -> io::Result<(libc::dev_t, Option<EntryKind>)> {
    let mut st = MaybeUninit::<libc::stat>::uninit();
    // SAFETY: `name` is NUL-terminated and `st` is writable storage for one stat
    let rc = unsafe {
        libc::fstatat(
            dirfd,
            name.as_ptr(),
            st.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if rc < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fstatat returned success, so it initialized `st`
    let st = unsafe { st.assume_init() };
    let mode = st.st_mode as u32 & libc::S_IFMT as u32;
    let kind = match mode {
        m if m == libc::S_IFLNK as u32 => None,
        m if m == libc::S_IFDIR as u32 => Some(EntryKind::Dir),
        m if m == libc::S_IFREG as u32 => Some(EntryKind::File),
        _ => Some(EntryKind::Other),
    };
    Ok((st.st_dev, kind))
}

fn check(rc: c_int) -> Result<(), FsError> {
    if rc < 0 {
        Err(io::Error::last_os_error().into())
    } else {
        Ok(())
    }
}

fn c_name(name: &Name) -> CString {
    CString::new(name.as_os_str().as_bytes()).expect("validated names contain no NUL")
}

/// Joins `rel` into a single `/`-separated relative path (`.` when empty).
fn c_path(rel: &RelPath) -> CString {
    let mut bytes = Vec::new();
    for name in rel.names() {
        if !bytes.is_empty() {
            bytes.push(b'/');
        }
        bytes.extend_from_slice(name.as_os_str().as_bytes());
    }
    if bytes.is_empty() {
        bytes.push(b'.');
    }
    CString::new(bytes).expect("validated names contain no NUL")
}

fn c_bytes(name: &[u8]) -> CString {
    CString::new(name).expect("directory entry names contain no NUL")
}

fn errno_set(value: c_int) {
    // SAFETY: __error() returns a valid pointer to this thread's errno
    unsafe {
        *libc::__error() = value;
    }
}

fn errno_is_zero() -> bool {
    // SAFETY: as above
    unsafe { *libc::__error() == 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::os::unix::fs::symlink;

    fn rel(path: &str) -> RelPath {
        match crate::capability::path::parse_request(Path::new(path)) {
            Ok(crate::capability::path::Request::Relative(rel)) => rel,
            other => panic!("{path}: {other:?}"),
        }
    }

    #[test]
    fn symlinks_below_the_root_are_never_followed() {
        let d = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), b"s").unwrap();
        std::fs::write(d.path().join("data.txt"), b"x").unwrap();
        std::fs::create_dir(d.path().join("dir")).unwrap();
        std::fs::write(d.path().join("dir/inner.txt"), b"y").unwrap();
        symlink("data.txt", d.path().join("file_link")).unwrap();
        symlink("dir", d.path().join("dir_link")).unwrap();
        symlink(outside.path(), d.path().join("escape")).unwrap();
        let root = open_root(d.path()).unwrap();

        // A trailing-component link, an interior link, and an escape link are
        // all refused; the real files still open.
        for path in ["file_link", "dir_link/inner.txt", "escape/secret.txt"] {
            assert!(
                matches!(
                    open_file(&root, &rel(path)),
                    Err(FsError::PathDenied(Denial::ObjectLocation))
                ),
                "{path} was followed"
            );
        }
        assert!(open_file(&root, &rel("data.txt")).is_ok());
        assert!(open_file(&root, &rel("dir/inner.txt")).is_ok());
        assert!(matches!(
            open_dir(&root, &rel("dir_link")),
            Err(FsError::PathDenied(Denial::ObjectLocation))
        ));

        // Listings omit the links.
        let dir = open_dir(&root, &RelPath::default()).unwrap();
        let names: Vec<_> = entries(&dir)
            .unwrap()
            .into_iter()
            .map(|e| e.name().to_owned())
            .collect();
        assert!(names.iter().any(|n| n == "data.txt"));
        assert!(
            !names
                .iter()
                .any(|n| n == "file_link" || n == "dir_link" || n == "escape")
        );
    }

    #[test]
    fn a_symlink_loop_fails_closed_instead_of_hanging() {
        let d = tempfile::tempdir().unwrap();
        symlink("b", d.path().join("a")).unwrap();
        symlink("a", d.path().join("b")).unwrap();
        let root = open_root(d.path()).unwrap();
        assert!(open_file(&root, &rel("a")).is_err());
    }

    #[test]
    fn reading_a_fifo_fails_instead_of_blocking() {
        let d = tempfile::tempdir().unwrap();
        let fifo = CString::new(d.path().join("pipe").as_os_str().as_bytes()).unwrap();
        // SAFETY: `fifo` is a NUL-terminated path
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        let root = open_root(d.path()).unwrap();
        assert!(matches!(
            open_file(&root, &rel("pipe")),
            Err(FsError::Io(_))
        ));
    }

    #[test]
    fn locations_come_from_f_getpath() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("report.txt"), b"x").unwrap();
        let root = open_root(d.path()).unwrap();
        let file = open_file(&root, &rel("report.txt")).unwrap();
        assert_eq!(location(&file).unwrap().file_name().unwrap(), "report.txt");
    }

    #[test]
    fn mutations_run_on_the_parent_handle() {
        use std::io::Write;
        let d = tempfile::tempdir().unwrap();
        let root = open_root(d.path()).unwrap();
        let dir = open_dir(&root, &RelPath::default()).unwrap();

        let name = Name::parse(OsStr::new("made.txt")).unwrap();
        let mut file = create_file(&dir, &name, Create::New).unwrap();
        file.write_all(b"hi").unwrap();
        drop(file);
        assert_eq!(std::fs::read(d.path().join("made.txt")).unwrap(), b"hi");

        let mut contents = String::new();
        open_file(&root, &rel("made.txt"))
            .unwrap()
            .read_to_string(&mut contents)
            .unwrap();
        assert_eq!(contents, "hi");

        let (entry, kind) = open_entry(&dir, &name).unwrap();
        assert_eq!(kind, EntryKind::File);
        remove(&dir, entry, kind).unwrap();
        assert!(!d.path().join("made.txt").exists());
    }
}

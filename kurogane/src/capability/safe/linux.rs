//! Linux backend for filesystem access confined beneath a root fd.
//!
//! Path resolution is performed beneath the root fd and never follows
//! symlinks, magic links, absolute path escapes or mount points. On kernels
//! with `openat2(2)`, lookups use `RESOLVE_BENEATH | RESOLVE_NO_MAGICLINKS |
//! RESOLVE_NO_SYMLINKS | RESOLVE_NO_XDEV`, retrying a bounded number of
//! times on `EAGAIN`; leaf opens of a mutation use the same rules. Entries
//! that are mount roots are refused and omitted from listings.
//!
//! Kernels before 5.6 fall back to a component-by-component
//! `openat(O_NOFOLLOW)` walk that compares each component's mount id with the
//! root's. Each component is a validated single name; mutations refuse when
//! `openat2` is unavailable.
//!
//! Reads and existing-file writes require regular files and use `O_NONBLOCK`
//! to prevent FIFOs from blocking. Directory lookups distinguish symlinks
//! that report `ENOTDIR` from ordinary missing directories. Object locations
//! are resolved through `/proc/self/fd`.

use std::ffi::{CStr, CString, OsStr};
use std::fs::{File, OpenOptions};
use std::io;
use std::mem::MaybeUninit;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use libc::{c_int, c_void, mode_t};

use super::{Create, DirEntry, EntryKind};
use crate::capability::error::{Denial, FsError};
use crate::capability::path::{Name, RelPath};

/// `struct open_how` of `openat2(2)`.
#[repr(C)]
struct OpenHow {
    flags: u64,
    mode: u64,
    resolve: u64,
}

const RESOLVE_NO_XDEV: u64 = 0x01;
const RESOLVE_NO_MAGICLINKS: u64 = 0x02;
const RESOLVE_NO_SYMLINKS: u64 = 0x04;
const RESOLVE_BENEATH: u64 = 0x08;
/// Mount points are links too: never crossed, bind mounts included.
const RESOLVE: u64 =
    RESOLVE_BENEATH | RESOLVE_NO_MAGICLINKS | RESOLVE_NO_SYMLINKS | RESOLVE_NO_XDEV;

/// `STATX_ATTR_MOUNT_ROOT` (Linux 5.8+): the entry is the root of a mount.
const STATX_ATTR_MOUNT_ROOT: u64 = 0x2000;

const EAGAIN_RETRIES: usize = 8;

/// An entry held for removal or renaming: its name within the parent fd.
pub(crate) struct Entry {
    leaf: Name,
}

pub(super) fn open_root(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_PATH | libc::O_DIRECTORY)
        .open(path)
}

pub(super) fn location(file: &File) -> io::Result<PathBuf> {
    let path = std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd()))?;
    // The kernel appends " (deleted)" to the last name of an object unlinked
    // since it was opened. Such an object has no location: fail closed. A
    // live object whose name really ends that way is itself at that path.
    if path.as_os_str().as_bytes().ends_with(b" (deleted)") {
        let object = file.metadata()?;
        let named = std::fs::symlink_metadata(&path)
            .is_ok_and(|entry| entry.dev() == object.dev() && entry.ino() == object.ino());
        if !named {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "the object was removed after it was opened",
            ));
        }
    }
    Ok(path)
}

pub(super) fn link_count(file: &File) -> io::Result<u64> {
    Ok(file.metadata()?.nlink())
}

pub(super) fn open_file(root: &File, rel: &RelPath) -> Result<File, FsError> {
    let fd = lookup(
        root,
        rel,
        libc::O_RDONLY | libc::O_NONBLOCK | libc::O_NOFOLLOW,
    )?;
    regular(File::from(fd))
}

pub(super) fn probe(root: &File, rel: &RelPath) -> Result<File, FsError> {
    let file = File::from(lookup(root, rel, libc::O_PATH | libc::O_NOFOLLOW)?);
    // With O_PATH | O_NOFOLLOW a trailing symlink opens as itself
    if file.metadata()?.file_type().is_symlink() {
        return Err(FsError::PathDenied(Denial::ObjectLocation));
    }
    Ok(file)
}

pub(super) fn open_dir(root: &File, rel: &RelPath) -> Result<File, FsError> {
    match lookup(
        root,
        rel,
        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW,
    ) {
        Ok(fd) => Ok(File::from(fd)),
        // With O_DIRECTORY | O_NOFOLLOW a trailing symlink fails with ENOTDIR
        // before the kernel reports the link. Look again, for metadata only,
        // to tell a link (a denial) from a plain file (an I/O error)
        Err(FsError::Io(e)) if e.raw_os_error() == Some(libc::ENOTDIR) => match probe(root, rel) {
            Err(denied @ FsError::PathDenied(_)) => Err(denied),
            _ => Err(FsError::Io(e)),
        },
        Err(e) => Err(e),
    }
}

pub(super) fn entries(dir: &File) -> io::Result<Vec<DirEntry>> {
    let mut entries = Vec::new();
    for (name, d_type) in read_dir_names(dir)? {
        if name.as_bytes() == b"." || name.as_bytes() == b".." {
            continue;
        }
        let kind = match d_type {
            libc::DT_DIR => EntryKind::Dir,
            libc::DT_REG => EntryKind::File,
            libc::DT_LNK => continue,
            libc::DT_UNKNOWN => match stat_kind(dir.as_raw_fd(), &name) {
                Ok(Some(kind)) => kind,
                // A link, or an entry that vanished since it was listed
                Ok(None) | Err(_) => continue,
            },
            _ => EntryKind::Other,
        };
        // A mount point is a link to another filesystem: omitted like one
        if !matches!(is_mount_root(dir.as_raw_fd(), &name), Ok(false)) {
            continue;
        }
        entries.push(DirEntry::new(
            OsStr::from_bytes(name.as_bytes()).to_owned(),
            kind,
        ));
    }
    Ok(entries)
}

pub(super) fn create_file(dir: &File, leaf: &Name, mode: Create) -> Result<File, FsError> {
    let name = c_name(leaf);
    match mode {
        Create::New => {
            let flags = libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW;
            leaf_open(dir, &name, flags, 0o644).map(File::from)
        }
        Create::Existing => {
            let flags = libc::O_WRONLY | libc::O_NONBLOCK | libc::O_NOFOLLOW;
            regular(File::from(leaf_open(dir, &name, flags, 0)?))
        }
    }
}

/// Opens one name in `dir` under the same rules as a lookup, so a mount
/// point at the leaf (a bind-mounted file) is refused like a link.
fn leaf_open(dir: &File, name: &CStr, flags: c_int, mode: mode_t) -> Result<OwnedFd, FsError> {
    if openat2_supported() {
        return openat2(dir.as_raw_fd(), name, flags, mode);
    }
    let fd = openat(dir.as_raw_fd(), name, flags, mode)?;
    same_mount(dir.as_raw_fd(), fd.as_raw_fd())?;
    Ok(fd)
}

pub(super) fn create_dir(dir: &File, leaf: &Name) -> Result<(), FsError> {
    require_openat2()?;
    let name = c_name(leaf);
    // SAFETY: `name` is NUL-terminated and outlives the call
    let rc = unsafe { libc::mkdirat(dir.as_raw_fd(), name.as_ptr(), 0o755) };
    check(rc)
}

pub(super) fn open_entry(dir: &File, leaf: &Name) -> Result<(Entry, EntryKind), FsError> {
    let name = c_name(leaf);
    let kind = stat_kind(dir.as_raw_fd(), &name)?;
    if is_mount_root(dir.as_raw_fd(), &name)? {
        return Err(FsError::PathDenied(Denial::ObjectLocation));
    }
    match kind {
        Some(kind) => Ok((Entry { leaf: leaf.clone() }, kind)),
        None => Err(FsError::PathDenied(Denial::ObjectLocation)),
    }
}

pub(super) fn entry_location(dir: &File, entry: &Entry) -> io::Result<PathBuf> {
    Ok(location(dir)?.join(entry.leaf.as_os_str()))
}

pub(super) fn remove(dir: &File, entry: Entry, kind: EntryKind) -> Result<(), FsError> {
    require_openat2()?;
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
    require_openat2()?;
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

fn lookup(root: &File, rel: &RelPath, flags: c_int) -> Result<OwnedFd, FsError> {
    if openat2_supported() {
        openat2(root.as_raw_fd(), &c_path(rel), flags, 0)
    } else {
        walk(root, rel, flags)
    }
}

fn openat2(dirfd: RawFd, path: &CStr, flags: c_int, mode: mode_t) -> Result<OwnedFd, FsError> {
    let how = OpenHow {
        flags: (flags | libc::O_CLOEXEC) as u64,
        mode: u64::from(mode),
        resolve: RESOLVE,
    };
    for _ in 0..EAGAIN_RETRIES {
        // SAFETY: `path` is NUL-terminated and `how` is a valid `open_how` of
        // the size passed; the kernel reads both only during the call
        let rc = unsafe {
            libc::syscall(
                libc::SYS_openat2,
                dirfd,
                path.as_ptr(),
                &how as *const OpenHow as *const c_void,
                size_of::<OpenHow>(),
            )
        };
        if rc >= 0 {
            // SAFETY: a non-negative return is a new descriptor we now own
            return Ok(unsafe { OwnedFd::from_raw_fd(rc as RawFd) });
        }
        let err = io::Error::last_os_error();
        if err.raw_os_error() != Some(libc::EAGAIN) {
            return Err(lookup_error(err));
        }
    }
    Err(io::Error::new(io::ErrorKind::TimedOut, "path resolution raced repeatedly").into())
}

/// The pre-5.6 lookup: one `openat(O_NOFOLLOW)` per component, each on the
/// root's mount (`st_dev` would misjudge btrfs subvolumes and overlayfs).
fn walk(root: &File, rel: &RelPath, flags: c_int) -> Result<OwnedFd, FsError> {
    let Some((last, parents)) = rel.names().split_last() else {
        return openat(root.as_raw_fd(), c".", flags, 0);
    };
    let mount = mount_id(root.as_raw_fd())?;
    let on_root_mount = |fd: RawFd| -> Result<(), FsError> {
        if mount_id(fd)? == mount {
            Ok(())
        } else {
            Err(FsError::PathDenied(Denial::ObjectLocation))
        }
    };
    let mut dir: Option<OwnedFd> = None;
    for name in parents {
        let dirfd = dir.as_ref().map_or(root.as_raw_fd(), AsRawFd::as_raw_fd);
        let next = File::from(openat(
            dirfd,
            &c_name(name),
            libc::O_PATH | libc::O_NOFOLLOW,
            0,
        )?);
        let file_type = next.metadata()?.file_type();
        if file_type.is_symlink() {
            return Err(FsError::PathDenied(Denial::ObjectLocation));
        }
        if !file_type.is_dir() {
            return Err(io::Error::from(io::ErrorKind::NotADirectory).into());
        }
        on_root_mount(next.as_raw_fd())?;
        dir = Some(OwnedFd::from(next));
    }
    let dirfd = dir.as_ref().map_or(root.as_raw_fd(), AsRawFd::as_raw_fd);
    let fd = openat(dirfd, &c_name(last), flags | libc::O_NOFOLLOW, 0)?;
    on_root_mount(fd.as_raw_fd())?;
    Ok(fd)
}

/// The mount id of `fd`, from `/proc/self/fdinfo`.
fn mount_id(fd: RawFd) -> io::Result<u64> {
    let info = std::fs::read_to_string(format!("/proc/self/fdinfo/{fd}"))?;
    info.lines()
        .find_map(|line| line.strip_prefix("mnt_id:"))
        .and_then(|id| id.trim().parse().ok())
        .ok_or_else(|| io::Error::other("fdinfo has no mnt_id"))
}

/// Refuses `fd` unless it lies on the same mount as `dir`.
fn same_mount(dir: RawFd, fd: RawFd) -> Result<(), FsError> {
    if mount_id(dir)? == mount_id(fd)? {
        Ok(())
    } else {
        Err(FsError::PathDenied(Denial::ObjectLocation))
    }
}

/// Whether `name` in `dirfd` is the root of a mount (without following it).
/// Kernels before 5.8 do not report it; their mutations of a mount point
/// fail with `EBUSY` and their opens are refused by `RESOLVE_NO_XDEV`.
fn is_mount_root(dirfd: RawFd, name: &CStr) -> io::Result<bool> {
    let mut stx = MaybeUninit::<libc::statx>::zeroed();
    // SAFETY: `name` is NUL-terminated and `stx` is writable storage for one
    // `struct statx`; the kernel only writes it during the call
    let rc = unsafe {
        libc::syscall(
            libc::SYS_statx,
            dirfd,
            name.as_ptr(),
            libc::AT_SYMLINK_NOFOLLOW | libc::AT_NO_AUTOMOUNT,
            0u32,
            stx.as_mut_ptr(),
        )
    };
    if rc < 0 {
        let err = io::Error::last_os_error();
        return if err.raw_os_error() == Some(libc::ENOSYS) {
            Ok(false)
        } else {
            Err(err)
        };
    }
    // SAFETY: statx succeeded, so it initialized `stx`
    let stx = unsafe { stx.assume_init() };
    Ok(stx.stx_attributes_mask & STATX_ATTR_MOUNT_ROOT != 0
        && stx.stx_attributes & STATX_ATTR_MOUNT_ROOT != 0)
}

fn openat(dirfd: RawFd, name: &CStr, flags: c_int, mode: mode_t) -> Result<OwnedFd, FsError> {
    // SAFETY: `name` is NUL-terminated and outlives the call
    let rc = unsafe { libc::openat(dirfd, name.as_ptr(), flags | libc::O_CLOEXEC, mode) };
    if rc < 0 {
        return Err(lookup_error(io::Error::last_os_error()));
    }
    // SAFETY: a non-negative return is a new descriptor we now own
    Ok(unsafe { OwnedFd::from_raw_fd(rc) })
}

/// `ELOOP` means a link was met (`RESOLVE_NO_SYMLINKS`, `O_NOFOLLOW`) and
/// `EXDEV` that resolution tried to leave the root or cross a mount point
/// (`RESOLVE_BENEATH`, `RESOLVE_NO_XDEV`): confinement verdicts, not I/O
/// failures.
fn lookup_error(err: io::Error) -> FsError {
    match err.raw_os_error() {
        Some(libc::ELOOP | libc::EXDEV) => FsError::PathDenied(Denial::ObjectLocation),
        _ => FsError::Io(err),
    }
}

/// Requires a regular file, then clears the `O_NONBLOCK` that only kept
/// `open` from waiting on a FIFO.
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

/// The kind of `name` in `dirfd` without following it; `None` for a link.
fn stat_kind(dirfd: RawFd, name: &CStr) -> io::Result<Option<EntryKind>> {
    let mut st = MaybeUninit::<libc::stat>::uninit();
    // SAFETY: `name` is NUL-terminated and `st` is writable storage for one `struct stat`
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
    let mode = unsafe { st.assume_init() }.st_mode & libc::S_IFMT;
    Ok(match mode {
        libc::S_IFLNK => None,
        libc::S_IFDIR => Some(EntryKind::Dir),
        libc::S_IFREG => Some(EntryKind::File),
        _ => Some(EntryKind::Other),
    })
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

fn openat2_supported() -> bool {
    static SUPPORTED: OnceLock<bool> = OnceLock::new();
    *SUPPORTED.get_or_init(|| {
        let how = OpenHow {
            flags: (libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC) as u64,
            mode: 0,
            resolve: RESOLVE,
        };
        // Kernels 5.6+ know every RESOLVE flag used here
        // SAFETY: "." is a static NUL-terminated path and `how` is a valid
        // `open_how` of the size passed
        let rc = unsafe {
            libc::syscall(
                libc::SYS_openat2,
                libc::AT_FDCWD,
                c".".as_ptr(),
                &how as *const OpenHow as *const c_void,
                size_of::<OpenHow>(),
            )
        };
        if rc >= 0 {
            // SAFETY: the probe descriptor is ours and closed exactly once
            drop(unsafe { OwnedFd::from_raw_fd(rc as RawFd) });
            return true;
        }
        // Any error other than ENOSYS means the syscall exists
        io::Error::last_os_error().raw_os_error() != Some(libc::ENOSYS)
    })
}

fn require_openat2() -> Result<(), FsError> {
    if openat2_supported() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "this mutation requires openat2 (Linux 5.6+)",
        )
        .into())
    }
}

/// Entry names and `d_type`s via `getdents64(2)`. End of directory is a zero
/// byte count, which (unlike `readdir(3)`) never confuses a stale `errno`
/// with an error.
fn read_dir_names(dir: &File) -> io::Result<Vec<(CString, u8)>> {
    /// `struct linux_dirent64`: d_ino u64, d_off i64, d_reclen u16, d_type u8, then the name
    const RECLEN: usize = 16;
    const TYPE: usize = 18;
    const NAME: usize = 19;

    let mut buf = vec![0u8; 16 * 1024];
    let mut names = Vec::new();
    loop {
        // SAFETY: `buf` is writable for `buf.len()` bytes for the whole call
        let rc = unsafe {
            libc::syscall(
                libc::SYS_getdents64,
                dir.as_raw_fd(),
                buf.as_mut_ptr() as *mut c_void,
                buf.len(),
            )
        };
        if rc < 0 {
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            return Err(err);
        }
        if rc == 0 {
            return Ok(names);
        }
        let filled = &buf[..rc as usize];
        let mut offset = 0;
        while offset + NAME <= filled.len() {
            let reclen = usize::from(u16::from_ne_bytes([
                filled[offset + RECLEN],
                filled[offset + RECLEN + 1],
            ]));
            let Some(record) = filled
                .get(offset..offset + reclen)
                .filter(|_| reclen > NAME)
            else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "malformed getdents64 record",
                ));
            };
            let name = &record[NAME..];
            let len = name.iter().position(|&b| b == 0).unwrap_or(name.len());
            let name = CString::new(&name[..len]).expect("NUL-free by construction");
            names.push((name, record[TYPE]));
            offset += reclen;
        }
    }
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
        std::fs::write(d.path().join("data.txt"), b"x").unwrap();
        std::fs::create_dir(d.path().join("dir")).unwrap();
        symlink("data.txt", d.path().join("file_link")).unwrap();
        symlink("dir", d.path().join("dir_link")).unwrap();
        let root = open_root(d.path()).unwrap();

        for path in ["file_link", "dir_link/anything"] {
            assert!(matches!(
                open_file(&root, &rel(path)),
                Err(FsError::PathDenied(Denial::ObjectLocation))
            ));
        }
        assert!(matches!(
            probe(&root, &rel("file_link")),
            Err(FsError::PathDenied(_))
        ));
        // A symlink opened as a directory is a denial; a plain file stays an
        // I/O error
        assert!(matches!(
            open_dir(&root, &rel("dir_link")),
            Err(FsError::PathDenied(Denial::ObjectLocation))
        ));
        assert!(matches!(
            open_dir(&root, &rel("data.txt")),
            Err(FsError::Io(ref e)) if e.raw_os_error() == Some(libc::ENOTDIR)
        ));
        let dir = open_dir(&root, &RelPath::default()).unwrap();
        let names: Vec<_> = entries(&dir)
            .unwrap()
            .into_iter()
            .map(|e| e.name().to_owned())
            .collect();
        assert!(names.iter().any(|n| n == "data.txt"));
        assert!(!names.iter().any(|n| n == "file_link" || n == "dir_link"));
    }

    #[test]
    fn the_component_walk_confines_like_openat2() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("dir")).unwrap();
        std::fs::write(d.path().join("dir/data.txt"), b"walk").unwrap();
        symlink("dir", d.path().join("dir_link")).unwrap();
        let root = open_root(d.path()).unwrap();

        let mut contents = String::new();
        File::from(walk(&root, &rel("dir/data.txt"), libc::O_RDONLY).unwrap())
            .read_to_string(&mut contents)
            .unwrap();
        assert_eq!(contents, "walk");
        assert!(matches!(
            walk(&root, &rel("dir_link/data.txt"), libc::O_RDONLY),
            Err(FsError::PathDenied(Denial::ObjectLocation))
        ));
    }

    #[test]
    fn deleted_objects_fail_closed() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("gone.txt"), b"x").unwrap();
        std::fs::write(d.path().join("kept (deleted)"), b"y").unwrap();
        let root = open_root(d.path()).unwrap();
        let gone = open_file(&root, &rel("gone.txt")).unwrap();
        std::fs::remove_file(d.path().join("gone.txt")).unwrap();
        assert!(
            location(&gone).is_err(),
            "a removed object reported a location"
        );
        let kept = open_file(&root, &rel("kept (deleted)")).unwrap();
        assert_eq!(
            location(&kept).unwrap().file_name().unwrap(),
            "kept (deleted)",
            "a live name ending in \" (deleted)\" is reported verbatim"
        );
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
}

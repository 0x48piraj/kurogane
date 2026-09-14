//! Windows backend for filesystem access confined beneath a directory handle.
//!
//! Path resolution is performed by NT relative to the root or parent handle.
//! Validated path components contain no `..`, and `OBJ_DONT_REPARSE` prevents
//! traversal through symlinks, junctions and mount points, including at the
//! final component.
//!
//! `OBJ_CASE_INSENSITIVE` preserves Win32 name semantics. Object locations
//! come from `GetFinalPathNameByHandleW`, providing the long, on-disk path
//! used by the authorization layer to catch aliases such as 8.3 names.
//!
//! Removals and renames operate on the verified entry handle, so the object
//! that was authorized is the object that is modified. Data reparse points
//! handled by the filesystem filter stack, such as WOF compression, dedup
//! and cloud placeholders, are not treated as reparse-point traversal.
//!
//! Every `unsafe` block is limited to an FFI call or buffer view and
//! documents the validity of its pointers.

use std::ffi::{c_void, OsString};
use std::fs::{File, OpenOptions};
use std::io;
use std::mem::{offset_of, size_of, size_of_val};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::ptr;

use windows_sys::Wdk::Foundation::OBJECT_ATTRIBUTES;
use windows_sys::Wdk::Storage::FileSystem::{
    FILE_CREATE, FILE_DIRECTORY_FILE, FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_INFORMATION_CLASS,
    FILE_RENAME_INFORMATION, FILE_RENAME_POSIX_SEMANTICS, FILE_RENAME_REPLACE_IF_EXISTS,
    FILE_SYNCHRONOUS_IO_NONALERT, FileRenameInformation, FileRenameInformationEx,
    NTCREATEFILE_CREATE_DISPOSITION, NTCREATEFILE_CREATE_OPTIONS, NtCreateFile,
    NtSetInformationFile,
};
use windows_sys::Win32::Foundation::{
    ERROR_INVALID_PARAMETER, ERROR_NO_MORE_FILES, ERROR_NOT_SUPPORTED, HANDLE, NTSTATUS,
    OBJ_CASE_INSENSITIVE, OBJ_DONT_REPARSE, RtlNtStatusToDosError, STATUS_INVALID_INFO_CLASS,
    STATUS_INVALID_PARAMETER, STATUS_NOT_SUPPORTED, STATUS_REPARSE_POINT_ENCOUNTERED,
    UNICODE_STRING,
};
use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_ACCESS_RIGHTS, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL,
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_DISPOSITION_FLAG_DELETE,
    FILE_DISPOSITION_FLAG_POSIX_SEMANTICS, FILE_DISPOSITION_INFO, FILE_DISPOSITION_INFO_EX,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_ID_BOTH_DIR_INFO,
    FILE_INFO_BY_HANDLE_CLASS, FILE_LIST_DIRECTORY, FILE_NAME_NORMALIZED, FILE_READ_ATTRIBUTES,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_TRAVERSE, FileDispositionInfo,
    FileDispositionInfoEx, FileIdBothDirectoryInfo, FileIdBothDirectoryRestartInfo,
    GetFileInformationByHandleEx, GetFinalPathNameByHandleW, SYNCHRONIZE,
    SetFileInformationByHandle, VOLUME_NAME_DOS,
};
use windows_sys::Win32::System::IO::{IO_STATUS_BLOCK, IO_STATUS_BLOCK_0};

use super::{Create, DirEntry, EntryKind};
use crate::capability::error::{Denial, FsError};
use crate::capability::path::{Name, RelPath};

const SHARE_ALL: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
const DIR_ACCESS: FILE_ACCESS_RIGHTS =
    FILE_LIST_DIRECTORY | FILE_TRAVERSE | FILE_READ_ATTRIBUTES | SYNCHRONIZE;
const ENTRY_ACCESS: FILE_ACCESS_RIGHTS = DELETE | FILE_READ_ATTRIBUTES | SYNCHRONIZE;

/// Reparse tags with this bit redirect name resolution (symlinks, junctions).
const TAG_NAME_SURROGATE: u32 = 0x2000_0000;

/// An entry held for removal or renaming: a handle opened with `DELETE`.
pub(crate) struct Entry {
    handle: File,
}

pub(super) fn open_root(path: &Path) -> io::Result<File> {
    let root = OpenOptions::new()
        .access_mode(DIR_ACCESS)
        .share_mode(SHARE_ALL)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?;
    if !root.metadata()?.is_dir() {
        return Err(io::Error::from(io::ErrorKind::NotADirectory));
    }
    Ok(root)
}

pub(super) fn location(file: &File) -> io::Result<PathBuf> {
    let mut buf = vec![0u16; 512];
    loop {
        let capacity = u32::try_from(buf.len()).expect("path buffers stay far below 4 GiB");
        // SAFETY: `buf` is writable for `capacity` UTF-16 units and the handle
        // is open for the duration of the call
        let len = unsafe {
            GetFinalPathNameByHandleW(
                raw(file),
                buf.as_mut_ptr(),
                capacity,
                FILE_NAME_NORMALIZED | VOLUME_NAME_DOS,
            )
        } as usize;
        if len == 0 {
            return Err(io::Error::last_os_error());
        }
        if len < buf.len() {
            return Ok(PathBuf::from(OsString::from_wide(&buf[..len])));
        }
        // Too small; `len` is the size needed, terminator included
        buf.resize(len, 0);
    }
}

pub(super) fn open_file(root: &File, rel: &RelPath) -> Result<File, FsError> {
    nt_open(
        root,
        &wide_path(rel),
        FILE_GENERIC_READ,
        FILE_OPEN,
        FILE_NON_DIRECTORY_FILE,
    )
}

pub(super) fn probe(root: &File, rel: &RelPath) -> Result<File, FsError> {
    nt_open(
        root,
        &wide_path(rel),
        FILE_READ_ATTRIBUTES | SYNCHRONIZE,
        FILE_OPEN,
        0,
    )
}

pub(super) fn open_dir(root: &File, rel: &RelPath) -> Result<File, FsError> {
    // An empty relative name reopens the root handle itself
    nt_open(
        root,
        &wide_path(rel),
        DIR_ACCESS,
        FILE_OPEN,
        FILE_DIRECTORY_FILE,
    )
}

pub(super) fn entries(dir: &File) -> io::Result<Vec<DirEntry>> {
    // u64 storage keeps the records 8-byte aligned, as the API requires
    let mut storage = vec![0u64; 8 * 1024];
    let byte_len = storage.len() * size_of::<u64>();
    let capacity = u32::try_from(byte_len).expect("a 64 KiB buffer fits in u32");
    let mut class = FileIdBothDirectoryRestartInfo;
    let mut entries = Vec::new();
    loop {
        // SAFETY: `storage` is writable for `capacity` bytes and the handle is
        // an open directory for the duration of the call
        let ok = unsafe {
            GetFileInformationByHandleEx(raw(dir), class, storage.as_mut_ptr().cast(), capacity)
        };
        if ok == 0 {
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
                return Ok(entries);
            }
            return Err(err);
        }
        class = FileIdBothDirectoryInfo;
        // SAFETY: `storage` is initialized memory of `byte_len` bytes; viewing
        // u64s as bytes is always valid
        let bytes = unsafe { std::slice::from_raw_parts(storage.as_ptr().cast::<u8>(), byte_len) };
        parse_records(bytes, &mut entries)?;
    }
}

pub(super) fn create_file(dir: &File, leaf: &Name, mode: Create) -> Result<File, FsError> {
    let disposition = match mode {
        Create::New => FILE_CREATE,
        Create::Existing => FILE_OPEN,
    };
    nt_open(
        dir,
        &wide_name(leaf),
        FILE_GENERIC_WRITE,
        disposition,
        FILE_NON_DIRECTORY_FILE,
    )
}

pub(super) fn create_dir(dir: &File, leaf: &Name) -> Result<(), FsError> {
    let access = FILE_LIST_DIRECTORY | SYNCHRONIZE;
    nt_open(
        dir,
        &wide_name(leaf),
        access,
        FILE_CREATE,
        FILE_DIRECTORY_FILE,
    )
    .map(drop)
}

pub(super) fn open_entry(dir: &File, leaf: &Name) -> Result<(Entry, EntryKind), FsError> {
    let handle = nt_open(dir, &wide_name(leaf), ENTRY_ACCESS, FILE_OPEN, 0)?;
    let kind = if handle.metadata()?.is_dir() {
        EntryKind::Dir
    } else {
        EntryKind::File
    };
    Ok((Entry { handle }, kind))
}

pub(super) fn entry_location(_dir: &File, entry: &Entry) -> io::Result<PathBuf> {
    location(&entry.handle)
}

pub(super) fn remove(_dir: &File, entry: Entry, _kind: EntryKind) -> Result<(), FsError> {
    let posix = FILE_DISPOSITION_INFO_EX {
        Flags: FILE_DISPOSITION_FLAG_DELETE | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS,
    };
    // SAFETY: `posix` is the structure `FileDispositionInfoEx` expects
    match unsafe { set_info(&entry.handle, FileDispositionInfoEx, &posix) } {
        Err(err) if unsupported_class(&err) => {
            // FAT and older NTFS: classic delete-on-close
            let classic = FILE_DISPOSITION_INFO { DeleteFile: true };
            // SAFETY: `classic` is the structure `FileDispositionInfo` expects
            unsafe { set_info(&entry.handle, FileDispositionInfo, &classic) }?;
            Ok(())
        }
        result => Ok(result?),
    }
}

pub(super) fn rename(_dir: &File, entry: Entry, to: &File, leaf: &Name) -> Result<(), FsError> {
    let name = wide_name(leaf);
    // Win32's SetFileInformationByHandle turns a rename target into an
    // absolute DOS path, so it cannot rename relative to `RootDirectory`; the
    // NT call it wraps can
    let header = offset_of!(FILE_RENAME_INFORMATION, FileName);
    let total = size_of::<FILE_RENAME_INFORMATION>() + size_of_val(name.as_slice());
    let name_bytes = u32::try_from(size_of_val(name.as_slice()))
        .map_err(|_| FsError::InvalidPath("path is too long"))?;
    let info_len = u32::try_from(total).map_err(|_| FsError::InvalidPath("path is too long"))?;

    // u64 storage keeps the header 8-byte aligned; it is zeroed, so the name
    // is followed by a NUL
    let mut storage = vec![0u64; total.div_ceil(size_of::<u64>())];
    let info = storage.as_mut_ptr().cast::<FILE_RENAME_INFORMATION>();
    let write = |flags: u32| {
        // SAFETY: `storage` is aligned for `FILE_RENAME_INFORMATION` and at least
        // `total` bytes long: the header plus the name written after it
        unsafe {
            (&raw mut (*info).Anonymous.Flags).write(flags);
            (&raw mut (*info).RootDirectory).write(raw(to));
            (&raw mut (*info).FileNameLength).write(name_bytes);
            let name_dst = info.cast::<u8>().add(header).cast::<u16>();
            ptr::copy_nonoverlapping(name.as_ptr(), name_dst, name.len());
        }
    };

    write(FILE_RENAME_REPLACE_IF_EXISTS | FILE_RENAME_POSIX_SEMANTICS);
    // SAFETY: `info` points to a complete `FILE_RENAME_INFORMATION` of
    // `info_len` bytes that outlives the call
    let mut status = unsafe {
        set_rename(
            &entry.handle,
            info.cast(),
            info_len,
            FileRenameInformationEx,
        )
    };
    if matches!(
        status,
        STATUS_INVALID_PARAMETER | STATUS_NOT_SUPPORTED | STATUS_INVALID_INFO_CLASS
    ) {
        // Before Windows 10 1709, or off NTFS: plain rename. The first byte
        // of `Flags` reads as `ReplaceIfExists`
        write(1);
        // SAFETY: as above
        status = unsafe { set_rename(&entry.handle, info.cast(), info_len, FileRenameInformation) };
    }
    if status < 0 {
        Err(nt_error(status))
    } else {
        Ok(())
    }
}

/// # Safety
///
/// `info` must point to `len` readable bytes holding a
/// `FILE_RENAME_INFORMATION` valid for `class`.
unsafe fn set_rename(
    file: &File,
    info: *const c_void,
    len: u32,
    class: FILE_INFORMATION_CLASS,
) -> NTSTATUS {
    let mut io_status = IO_STATUS_BLOCK {
        Anonymous: IO_STATUS_BLOCK_0 { Status: 0 },
        Information: 0,
    };
    // SAFETY: forwarded caller contract; `io_status` is a live local and the
    // handle is open
    unsafe { NtSetInformationFile(raw(file), &mut io_status, info, len, class) }
}

/// Opens `name` relative to `dir`. Reparse points on the way (final component
/// included) are refused by the kernel.
fn nt_open(
    dir: &File,
    name: &[u16],
    access: FILE_ACCESS_RIGHTS,
    disposition: NTCREATEFILE_CREATE_DISPOSITION,
    options: NTCREATEFILE_CREATE_OPTIONS,
) -> Result<File, FsError> {
    let byte_len =
        u16::try_from(size_of_val(name)).map_err(|_| FsError::InvalidPath("path is too long"))?;
    let object_name = UNICODE_STRING {
        Length: byte_len,
        MaximumLength: byte_len,
        Buffer: name.as_ptr().cast_mut(),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: raw(dir),
        ObjectName: &object_name,
        Attributes: OBJ_CASE_INSENSITIVE | OBJ_DONT_REPARSE,
        SecurityDescriptor: ptr::null(),
        SecurityQualityOfService: ptr::null(),
    };
    let mut io_status = IO_STATUS_BLOCK {
        Anonymous: IO_STATUS_BLOCK_0 { Status: 0 },
        Information: 0,
    };
    let mut handle: HANDLE = ptr::null_mut();
    // SAFETY: every pointer refers to a live local for the whole call; the
    // name's length is specified so it needs no terminator and NT only reads it
    let status = unsafe {
        NtCreateFile(
            &mut handle,
            access | SYNCHRONIZE,
            &attributes,
            &mut io_status,
            ptr::null(),
            FILE_ATTRIBUTE_NORMAL,
            SHARE_ALL,
            disposition,
            options | FILE_SYNCHRONOUS_IO_NONALERT,
            ptr::null(),
            0,
        )
    };
    if status < 0 {
        return Err(nt_error(status));
    }
    // SAFETY: NtCreateFile succeeded, so `handle` is a new handle we own
    Ok(File::from(unsafe { OwnedHandle::from_raw_handle(handle) }))
}

fn nt_error(status: NTSTATUS) -> FsError {
    if status == STATUS_REPARSE_POINT_ENCOUNTERED {
        return FsError::PathDenied(Denial::ObjectLocation);
    }
    // SAFETY: a pure status-code translation with no pointers
    let code = unsafe { RtlNtStatusToDosError(status) };
    FsError::Io(io::Error::from_raw_os_error(code as i32))
}

/// # Safety
///
/// `T` must be the structure `class` expects.
unsafe fn set_info<T>(file: &File, class: FILE_INFO_BY_HANDLE_CLASS, info: &T) -> io::Result<()> {
    let len = u32::try_from(size_of::<T>()).expect("information classes are small");
    // SAFETY: `info` is a live, initialized `T`; the caller guarantees it is
    // the layout `class` expects
    unsafe { set_info_raw(file, class, ptr::from_ref(info).cast(), len) }
}

/// # Safety
///
/// `info` must point to `len` readable bytes laid out as `class` expects.
unsafe fn set_info_raw(
    file: &File,
    class: FILE_INFO_BY_HANDLE_CLASS,
    info: *const c_void,
    len: u32,
) -> io::Result<()> {
    // SAFETY: forwarded caller contract; the handle is open
    let ok = unsafe { SetFileInformationByHandle(raw(file), class, info, len) };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// The `Ex` information classes need Windows 10 1709+ and NTFS.
fn unsupported_class(err: &io::Error) -> bool {
    matches!(err.raw_os_error(), Some(code) if code == ERROR_INVALID_PARAMETER as i32 || code == ERROR_NOT_SUPPORTED as i32)
}

fn raw(file: &File) -> HANDLE {
    file.as_raw_handle()
}

fn wide_name(name: &Name) -> Vec<u16> {
    name.as_os_str().encode_wide().collect()
}

fn wide_path(rel: &RelPath) -> Vec<u16> {
    let mut out = Vec::new();
    for name in rel.names() {
        if !out.is_empty() {
            out.push(u16::from(b'\\'));
        }
        out.extend(name.as_os_str().encode_wide());
    }
    out
}

/// Appends the entries of one `FILE_ID_BOTH_DIR_INFO` batch. Links (name
/// surrogate reparse points) and `.`/`..` are skipped.
fn parse_records(bytes: &[u8], entries: &mut Vec<DirEntry>) -> io::Result<()> {
    let field = |record: &[u8], offset: usize| -> io::Result<u32> {
        record
            .get(offset..offset + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "truncated directory record"))
    };
    let mut offset = 0;
    loop {
        let record = bytes.get(offset..).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "directory record out of range")
        })?;
        let next = field(record, offset_of!(FILE_ID_BOTH_DIR_INFO, NextEntryOffset))? as usize;
        let attributes = field(record, offset_of!(FILE_ID_BOTH_DIR_INFO, FileAttributes))?;
        let name_len = field(record, offset_of!(FILE_ID_BOTH_DIR_INFO, FileNameLength))? as usize;
        // For reparse points, EaSize holds the reparse tag
        let tag = field(record, offset_of!(FILE_ID_BOTH_DIR_INFO, EaSize))?;
        let start = offset_of!(FILE_ID_BOTH_DIR_INFO, FileName);
        let name_bytes = record
            .get(start..start + name_len)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "truncated file name"))?;
        let name: Vec<u16> = name_bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&pair| u16::from_le_bytes(pair))
            .collect();

        let dots = name == [u16::from(b'.')] || name == [u16::from(b'.'), u16::from(b'.')];
        let link = attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 && tag & TAG_NAME_SURROGATE != 0;
        if !dots && !link {
            let kind = if attributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
                EntryKind::Dir
            } else {
                EntryKind::File
            };
            entries.push(DirEntry::new(OsString::from_wide(&name), kind));
        }
        if next == 0 {
            return Ok(());
        }
        offset += next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::path::{parse_request, Request};
    use crate::capability::test_support::{link_dir, link_file};

    fn rel(path: &str) -> RelPath {
        match parse_request(Path::new(path)) {
            Ok(Request::Relative(rel)) => rel,
            other => panic!("{path}: {other:?}"),
        }
    }

    fn names(dir: &File) -> Vec<String> {
        entries(dir)
            .unwrap()
            .iter()
            .map(|e| e.name().to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn junctions_below_the_root_are_never_followed() {
        let d = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), b"s").unwrap();
        std::fs::create_dir(d.path().join("inside")).unwrap();
        std::fs::write(d.path().join("inside/ok.txt"), b"ok").unwrap();
        link_dir(outside.path(), &d.path().join("escape"));
        link_dir(&d.path().join("inside"), &d.path().join("alias"));
        let root = open_root(d.path()).unwrap();

        for path in ["escape/secret.txt", "alias/ok.txt"] {
            assert!(
                matches!(
                    open_file(&root, &rel(path)),
                    Err(FsError::PathDenied(Denial::ObjectLocation))
                ),
                "{path}"
            );
        }
        assert!(matches!(
            open_dir(&root, &rel("escape")),
            Err(FsError::PathDenied(_))
        ));
        assert!(matches!(
            probe(&root, &rel("alias")),
            Err(FsError::PathDenied(_))
        ));
        assert!(open_file(&root, &rel("inside/ok.txt")).is_ok());

        let listed = names(&open_dir(&root, &RelPath::default()).unwrap());
        assert!(listed.contains(&"inside".to_owned()));
        assert!(!listed.iter().any(|n| n == "escape" || n == "alias"));
    }

    #[test]
    fn file_symlinks_are_refused_when_they_can_be_created() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("data.txt"), b"x").unwrap();
        if !link_file(&d.path().join("data.txt"), &d.path().join("link.txt")) {
            eprintln!("skipped: this process cannot create symlinks (enable Developer Mode)");
            return;
        }
        let root = open_root(d.path()).unwrap();
        assert!(matches!(
            open_file(&root, &rel("link.txt")),
            Err(FsError::PathDenied(_))
        ));
        let dir = open_dir(&root, &RelPath::default()).unwrap();
        assert!(!names(&dir).contains(&"link.txt".to_owned()));
        assert!(matches!(
            dir_entry(&dir, "link.txt"),
            Err(FsError::PathDenied(_))
        ));
    }

    fn dir_entry(dir: &File, leaf: &str) -> Result<EntryKind, FsError> {
        let name = Name::parse(std::ffi::OsStr::new(leaf)).unwrap();
        open_entry(dir, &name).map(|(_, kind)| kind)
    }

    #[test]
    fn locations_report_long_names_in_disk_case() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("Report.txt"), b"x").unwrap();
        let root = open_root(d.path()).unwrap();
        let file = open_file(&root, &rel("REPORT.TXT")).unwrap();
        assert_eq!(location(&file).unwrap().file_name().unwrap(), "Report.txt");
    }

    #[test]
    fn listing_many_entries_spans_several_batches() {
        let d = tempfile::tempdir().unwrap();
        for i in 0..600 {
            std::fs::write(
                d.path().join(format!("file-with-a-longer-name-{i:04}.txt")),
                b"",
            )
            .unwrap();
        }
        let root = open_root(d.path()).unwrap();
        assert_eq!(
            names(&open_dir(&root, &RelPath::default()).unwrap()).len(),
            600
        );
    }
}

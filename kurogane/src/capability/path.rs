//! Path vocabulary of the capability layer.
//!
//! - [`Name`]: one validated component. Only names parse; nothing downstream
//!   re-validates strings.
//! - [`RelPath`]: a validated path beneath an allow root (empty = the root).
//! - [`Key`]: the comparison form of a component. Case-folded on Windows,
//!   whose filesystems are case-insensitive; raw bytes elsewhere. Folding can
//!   only make a deny rule match more, never less.
//! - [`Location`]: an absolute path as keys, used to compare roots, requests
//!   and kernel-reported object locations.
//!
//! [`parse_request`] is the only entry point for renderer-supplied paths.

use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, Prefix};

use crate::capability::error::{Denial, FsError};

/// Comparison key of one path component.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Key(Box<[u8]>);

impl Key {
    pub(crate) fn of(component: &OsStr) -> Key {
        Key(fold(component).into_boxed_slice())
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

/// Windows filesystems compare names through an upcase table built from
/// Unicode simple uppercase mappings. A char whose uppercase is several chars
/// (`ß` to `SS`) has no simple mapping and stays as is, like NTFS. Unpaired
/// surrogates keep their WTF-8 encoding.
#[cfg(windows)]
pub(crate) fn fold(component: &OsStr) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;

    let mut out = Vec::with_capacity(component.len());
    for unit in char::decode_utf16(component.encode_wide()) {
        match unit {
            Ok(c) => {
                let mut upper = c.to_uppercase();
                let folded = match (upper.next(), upper.next()) {
                    (Some(u), None) => u,
                    _ => c,
                };
                let mut buf = [0; 4];
                out.extend_from_slice(folded.encode_utf8(&mut buf).as_bytes());
            }
            Err(e) => {
                let u = e.unpaired_surrogate();
                out.extend_from_slice(&[
                    0xE0 | (u >> 12) as u8,
                    0x80 | ((u >> 6) & 0x3F) as u8,
                    0x80 | (u & 0x3F) as u8,
                ]);
            }
        }
    }
    out
}

#[cfg(not(windows))]
pub(crate) fn fold(component: &OsStr) -> Vec<u8> {
    component.as_encoded_bytes().to_vec()
}

/// Every character whose fold is `folded`, `folded` itself included, so a
/// pattern written against raw characters (a glob class range) can be
/// matched against folded names.
#[cfg(windows)]
pub(crate) fn unfold(folded: char) -> impl Iterator<Item = char> {
    use std::collections::HashMap;
    use std::sync::OnceLock;

    /// Simple uppercase, inverted: every character mapping to each target.
    static INVERSE: OnceLock<HashMap<char, Vec<char>>> = OnceLock::new();
    let inverse = INVERSE.get_or_init(|| {
        let mut inverse: HashMap<char, Vec<char>> = HashMap::new();
        for c in (0..=0x10_FFFF).filter_map(char::from_u32) {
            let mut upper = c.to_uppercase();
            if let (Some(u), None) = (upper.next(), upper.next())
                && u != c
            {
                inverse.entry(u).or_default().push(c);
            }
        }
        inverse
    });
    std::iter::once(folded).chain(inverse.get(&folded).into_iter().flatten().copied())
}

/// On macOS a character may fold to several (`É` to `e` + U+0301), so every
/// character whose fold *starts with* `folded` is included: a class naming
/// `É` then matches the `e` it decomposes to, and the combining mark after
/// it is absorbed by the glob matcher. Over-matching only denies more.
#[cfg(target_os = "macos")]
pub(crate) fn unfold(folded: char) -> impl Iterator<Item = char> {
    use std::collections::HashMap;
    use std::sync::OnceLock;

    static INVERSE: OnceLock<HashMap<char, Vec<char>>> = OnceLock::new();
    let inverse = INVERSE.get_or_init(|| {
        let mut inverse: HashMap<char, Vec<char>> = HashMap::new();
        let mut buf = [0; 4];
        for c in (0..=0x10_FFFF).filter_map(char::from_u32) {
            let bytes = fold(OsStr::new(c.encode_utf8(&mut buf)));
            let first = std::str::from_utf8(&bytes)
                .ok()
                .and_then(|s| s.chars().next());
            if let Some(first) = first
                && (first != c || bytes.len() != c.len_utf8())
            {
                inverse.entry(first).or_default().push(c);
            }
        }
        inverse
    });
    std::iter::once(folded).chain(inverse.get(&folded).into_iter().flatten().copied())
}

#[cfg(not(any(windows, target_os = "macos")))]
pub(crate) fn unfold(folded: char) -> impl Iterator<Item = char> {
    std::iter::once(folded)
}

/// One validated path component: not empty, not `.`/`..`, no separator, no
/// NUL. On Windows it is also a name Win32 neither rewrites nor reserves; no
/// `<>:"|?*` or control characters (`:` would address an alternate data
/// stream), no trailing dot or space, no device name.
#[derive(Clone, Debug)]
pub(crate) struct Name {
    raw: OsString,
    key: Key,
}

impl Name {
    pub(crate) fn parse(raw: &OsStr) -> Result<Name, &'static str> {
        let bytes = raw.as_encoded_bytes();
        if bytes.is_empty() {
            return Err("path contains an empty component");
        }
        if bytes.contains(&0) {
            return Err("path contains NUL");
        }
        if bytes == b"." || bytes == b".." || bytes.contains(&b'/') {
            return Err("path component is not a single name");
        }
        #[cfg(windows)]
        windows_name_rules(raw)?;
        Ok(Name {
            raw: raw.to_os_string(),
            key: Key::of(raw),
        })
    }

    pub(crate) fn as_os_str(&self) -> &OsStr {
        &self.raw
    }

    pub(crate) fn key(&self) -> &Key {
        &self.key
    }
}

#[cfg(windows)]
fn windows_name_rules(raw: &OsStr) -> Result<(), &'static str> {
    use std::os::windows::ffi::OsStrExt;

    const FORBIDDEN: &[u16] = &[
        b'<' as u16,
        b'>' as u16,
        b':' as u16,
        b'"' as u16,
        b'|' as u16,
        b'?' as u16,
        b'*' as u16,
        b'\\' as u16,
    ];
    let units: Vec<u16> = raw.encode_wide().collect();
    if units.iter().any(|&u| u < 0x20 || FORBIDDEN.contains(&u)) {
        return Err("path contains a character Windows does not allow in names");
    }
    if matches!(units.last(), Some(&u) if u == b'.' as u16 || u == b' ' as u16) {
        return Err("path component ends with a dot or space");
    }
    if is_device_name(raw) {
        return Err("path component is a reserved device name");
    }
    Ok(())
}

/// `CON`, `nul.txt`, `COM1 .log`: the part before the first dot, trailing
/// spaces removed, compared case-insensitively.
#[cfg(windows)]
fn is_device_name(raw: &OsStr) -> bool {
    const DEVICES: &[&str] = &["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"];
    let name = raw.to_string_lossy();
    let base = name
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end_matches(' ')
        .to_uppercase();
    if DEVICES.contains(&base.as_str()) {
        return true;
    }
    let mut chars = base.chars();
    let prefix: String = chars.by_ref().take(3).collect();
    let rest: Vec<char> = chars.collect();
    (prefix == "COM" || prefix == "LPT")
        && matches!(rest.as_slice(), [c] if c.is_ascii_digit() || matches!(c, '¹' | '²' | '³'))
}

/// A validated path beneath an allow root; empty means the root itself.
#[derive(Clone, Debug, Default)]
pub(crate) struct RelPath(Vec<Name>);

impl RelPath {
    pub(crate) fn names(&self) -> &[Name] {
        &self.0
    }

    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }

    /// The parent directory and the final name; `None` for the root itself.
    pub(crate) fn split_leaf(&self) -> Option<(RelPath, &Name)> {
        let (leaf, parent) = self.0.split_last()?;
        Some((RelPath(parent.to_vec()), leaf))
    }
}

/// An absolute path as keys. `volume` is empty on Unix, `C:` or
/// `\\SERVER\SHARE` (folded) on Windows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Location {
    volume: Key,
    keys: Vec<Key>,
}

impl Location {
    /// Parses a trusted absolute path: configuration or a kernel-reported
    /// object location. Verbatim `\\?\` forms are accepted and simplified;
    /// `None` for relative or device paths.
    pub(crate) fn parse(path: &Path) -> Option<Location> {
        let mut volume = None;
        let mut rooted = false;
        let mut keys = Vec::new();
        for component in path.components() {
            match component {
                Component::Prefix(prefix) => volume = Some(volume_key(prefix.kind(), true)?),
                Component::RootDir => rooted = true,
                Component::CurDir => {}
                Component::ParentDir => {
                    keys.pop();
                }
                Component::Normal(s) => keys.push(Key::of(s)),
            }
        }
        if !rooted || (cfg!(windows) && volume.is_none()) {
            return None;
        }
        Some(Location {
            volume: volume.unwrap_or_else(|| Key::of(OsStr::new(""))),
            keys,
        })
    }

    pub(crate) fn keys(&self) -> &[Key] {
        &self.keys
    }

    /// This location extended by `tail`.
    pub(crate) fn join<'k>(&self, tail: impl IntoIterator<Item = &'k Key>) -> Location {
        let mut keys = self.keys.clone();
        keys.extend(tail.into_iter().cloned());
        Location {
            volume: self.volume.clone(),
            keys,
        }
    }

    /// The keys of `self` beneath `base`, on a component boundary.
    pub(crate) fn strip(&self, base: &Location) -> Option<&[Key]> {
        if self.volume != base.volume || !self.keys.starts_with(&base.keys) {
            return None;
        }
        Some(&self.keys[base.keys.len()..])
    }

    /// The request names beneath this location, if the request lies under it.
    pub(crate) fn relative(&self, volume: &Key, names: &[Name]) -> Option<RelPath> {
        if &self.volume != volume || names.len() < self.keys.len() {
            return None;
        }
        let under = self.keys.iter().zip(names).all(|(k, n)| k == n.key());
        under.then(|| RelPath(names[self.keys.len()..].to_vec()))
    }
}

fn volume_key(prefix: Prefix<'_>, trusted: bool) -> Option<Key> {
    let text = match prefix {
        Prefix::Disk(letter) => format!("{}:", letter.to_ascii_uppercase() as char),
        Prefix::VerbatimDisk(letter) if trusted => {
            format!("{}:", letter.to_ascii_uppercase() as char)
        }
        Prefix::UNC(server, share) => {
            format!(
                r"\\{}\{}",
                server.to_string_lossy(),
                share.to_string_lossy()
            )
        }
        Prefix::VerbatimUNC(server, share) if trusted => {
            format!(
                r"\\{}\{}",
                server.to_string_lossy(),
                share.to_string_lossy()
            )
        }
        Prefix::Verbatim(_)
        | Prefix::VerbatimDisk(_)
        | Prefix::VerbatimUNC(..)
        | Prefix::DeviceNS(_) => {
            return None;
        }
    };
    Some(Key::of(OsStr::new(&text)))
}

/// The most components a single request may resolve to. Beyond this a
/// request is rejected before it reaches the scope matcher whose evaluation
/// is super-linear in path depth and runs on the single filesystem worker.
/// No legitimate path is anywhere near this deep; the cap bounds both the
/// matcher's cost and the per-name allocations one request can force.
pub(crate) const MAX_COMPONENTS: usize = 1024;

/// A renderer-supplied path after lexical normalization.
#[derive(Debug)]
pub(crate) enum Request {
    Absolute {
        volume: Key,
        names: Vec<Name>,
    },
    /// Anchored at the origin's single allow root.
    Relative(RelPath),
}

/// Parses a renderer-supplied path. `.` is dropped and `..` pops lexically;
/// a `..` popping above the start is outside every root. Verbatim, device and
/// drive-relative forms are rejected rather than normalized.
pub(crate) fn parse_request(path: &Path) -> Result<Request, FsError> {
    if path.as_os_str().is_empty() {
        return Err(FsError::InvalidPath("path is empty"));
    }
    if path.as_os_str().as_encoded_bytes().contains(&0) {
        return Err(FsError::InvalidPath("path contains NUL"));
    }

    let mut volume = None;
    let mut rooted = false;
    let mut names = Vec::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => {
                let key = volume_key(prefix.kind(), false).ok_or(FsError::InvalidPath(
                    "verbatim and device paths are not accepted",
                ))?;
                volume = Some(key);
            }
            Component::RootDir => rooted = true,
            Component::CurDir => {}
            Component::ParentDir => {
                if names.pop().is_none() {
                    return Err(FsError::PathDenied(Denial::OutsideRoots));
                }
            }
            Component::Normal(s) => {
                if names.len() >= MAX_COMPONENTS {
                    return Err(FsError::InvalidPath("path has too many components"));
                }
                names.push(Name::parse(s).map_err(FsError::InvalidPath)?);
            }
        }
    }

    match (volume, rooted) {
        (Some(_), false) => Err(FsError::InvalidPath(
            "drive-relative paths are not accepted",
        )),
        (Some(volume), true) => Ok(Request::Absolute { volume, names }),
        (None, true) if cfg!(windows) => Err(FsError::InvalidPath(
            "path must start with a drive or UNC share",
        )),
        (None, true) => Ok(Request::Absolute {
            volume: Key::of(OsStr::new("")),
            names,
        }),
        (None, false) => Ok(Request::Relative(RelPath(names))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn invalid(path: &str) -> bool {
        matches!(parse_request(Path::new(path)), Err(FsError::InvalidPath(_)))
    }

    #[test]
    fn relative_requests_normalize_lexically() {
        let Ok(Request::Relative(rel)) = parse_request(Path::new("a/./b/../c")) else {
            panic!("relative request");
        };
        let names: Vec<_> = rel
            .names()
            .iter()
            .map(|n| n.as_os_str().to_owned())
            .collect();
        assert_eq!(names, ["a", "c"]);
    }

    #[test]
    fn popping_above_the_start_is_outside_every_root() {
        for path in ["..", "../x", "a/../../x"] {
            assert!(
                matches!(
                    parse_request(Path::new(path)),
                    Err(FsError::PathDenied(Denial::OutsideRoots))
                ),
                "{path}"
            );
        }
    }

    #[test]
    fn empty_and_nul_are_invalid() {
        assert!(invalid(""));
        let nul = unsafe { OsString::from_encoded_bytes_unchecked(b"a\0b".to_vec()) };
        assert!(matches!(
            parse_request(&PathBuf::from(nul)),
            Err(FsError::InvalidPath(_))
        ));
    }

    #[test]
    fn location_strip_respects_component_boundaries() {
        let base = Location::parse(&std::env::temp_dir().join("notes")).unwrap();
        let inside = Location::parse(&std::env::temp_dir().join("notes").join("a")).unwrap();
        let sibling = Location::parse(&std::env::temp_dir().join("notes-evil")).unwrap();
        assert_eq!(inside.strip(&base).map(<[Key]>::len), Some(1));
        assert!(sibling.strip(&base).is_none());
    }

    #[test]
    fn too_many_components_are_rejected() {
        // A request deeper than the cap is rejected before it reaches the
        // scope matcher whose evaluation is super-linear in depth
        let over = vec!["a"; MAX_COMPONENTS + 1].join("/");
        assert!(invalid(&over));
        let at = vec!["a"; MAX_COMPONENTS].join("/");
        assert!(parse_request(Path::new(&at)).is_ok());
    }

    #[cfg(windows)]
    #[test]
    fn windows_rejects_aliasing_forms() {
        for path in [
            r"\\?\C:\data\x",
            r"\\.\C:\data\x",
            r"C:data\x",
            r"\data\x",
            r"C:\data\a.key:stream",
            r"C:\data\a.key::$DATA",
            r"C:\data\secret.",
            r"C:\data\secret ",
            r"C:\data\CON",
            r"C:\data\nul.txt",
            r"C:\data\COM1",
            r"C:\data\a*b",
        ] {
            assert!(invalid(path), "{path} must be rejected");
        }
        assert!(!invalid(r"C:\data\console.txt"));
        assert!(!invalid(r"C:\data\COM10"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_keys_fold_case_and_volumes() {
        assert_eq!(Key::of(OsStr::new("Secret")), Key::of(OsStr::new("SECRET")));
        assert_eq!(Key::of(OsStr::new("straße")), Key::of(OsStr::new("STRAßE")));
        let a = Location::parse(Path::new(r"c:\Data\X")).unwrap();
        let b = Location::parse(Path::new(r"\\?\C:\data\x")).unwrap();
        assert_eq!(a, b);
    }

    #[cfg(not(windows))]
    #[test]
    fn unix_keys_are_case_sensitive() {
        assert_ne!(Key::of(OsStr::new("Secret")), Key::of(OsStr::new("SECRET")));
    }
}

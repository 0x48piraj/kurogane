//! Path vocabulary of the capability layer.
//!
//! - [`Name`]: one validated component. Only names parse; nothing downstream
//!   re-validates strings.
//! - [`RelPath`]: a validated path beneath an allow root (empty = the root).
//! - [`Key`]: the comparison form of a component. Case-folded on Windows;
//!   case-, normalization- and ignorable-folded on macOS and Linux. Each fold
//!   identifies at least the names the platform's filesystems treat as one,
//!   so folding can only make a deny rule match more, never less.
//!   Kernel paths are always built from raw names, never from keys.
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

/// Normalizes a Windows path component using NTFS simple uppercase folding semantics.
///
/// Characters without a 1:1 simple uppercase mapping (e.g., `ß`) remain unmodified.
/// Unpaired surrogates are preserved as 3-byte WTF-8 sequences.
#[cfg(windows)]
pub(crate) fn fold(component: &OsStr) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;

    let mut out = Vec::with_capacity(component.len());
    for unit in char::decode_utf16(component.encode_wide()) {
        match unit {
            Ok(c) => {
                let mut buf = [0; 4];
                for folded in fold_char(c) {
                    out.extend_from_slice(folded.encode_utf8(&mut buf).as_bytes());
                }
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

/// The fold of one character: its simple uppercase when that is a single
/// character, else the character itself. Windows folds each character on its
/// own, so a name's fold is its characters' folds in order.
#[cfg(windows)]
fn fold_char(c: char) -> impl Iterator<Item = char> {
    let mut upper = c.to_uppercase();
    std::iter::once(match (upper.next(), upper.next()) {
        (Some(u), None) => u,
        _ => c,
    })
}

/// Folds a path component for filesystem name matching.
///
/// APFS and HFS+ are case- and normalization-insensitive by default. Linux
/// casefolded directories have the same property. The fold uses NFD
/// normalization, Unicode case conversion and removal of filesystem-ignored
/// code points.
///
/// Names that the filesystem treats as equal fold to the same bytes. In a
/// case-sensitive directory, the fold also makes names differing only in case
/// compare equal.
///
/// Non-UTF-8 names are returned unchanged.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) fn fold(component: &OsStr) -> Vec<u8> {
    let Some(text) = component.to_str() else {
        return component.as_encoded_bytes().to_vec();
    };
    fold_chars(text.chars()).collect::<String>().into_bytes()
}

/// The fold as a pipeline over characters. A name goes through it whole:
/// the last NFD reorders combining marks across characters.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn fold_chars(chars: impl Iterator<Item = char>) -> impl Iterator<Item = char> {
    use unicode_normalization::UnicodeNormalization;

    chars
        .nfd()
        .flat_map(char::to_uppercase)
        .flat_map(char::to_lowercase)
        .filter(|&c| !ignorable(c))
        .nfd()
}

/// The fold of a name made of `c` alone.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn fold_char(c: char) -> impl Iterator<Item = char> {
    fold_chars(std::iter::once(c))
}

/// Returns `true` if `c` is a Unicode `Default_Ignorable_Code_Point` stripped during
/// filename normalization on Linux casefold (`chattr +F`) and macOS (APFS/HFS+).
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn ignorable(c: char) -> bool {
    matches!(
        c,
        '\u{AD}'
            | '\u{34F}'
            | '\u{61C}'
            | '\u{115F}'..='\u{1160}'
            | '\u{17B4}'..='\u{17B5}'
            | '\u{180B}'..='\u{180F}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{206F}'
            | '\u{3164}'
            | '\u{FE00}'..='\u{FE0F}'
            | '\u{FEFF}'
            | '\u{FFA0}'
            | '\u{FFF0}'..='\u{FFF8}'
            | '\u{1BCA0}'..='\u{1BCA3}'
            | '\u{1D173}'..='\u{1D17A}'
            | '\u{E0000}'..='\u{E0FFF}'
    )
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub(crate) fn fold(component: &OsStr) -> Vec<u8> {
    component.as_encoded_bytes().to_vec()
}

/// The fold of a name made of `c` alone: names are not folded here.
#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn fold_char(c: char) -> impl Iterator<Item = char> {
    std::iter::once(c)
}

/// Yields `folded` and all Unicode code points whose fold begins with `folded`.
///
/// Inverts [`fold`] through the per-character fold the platform's `fold` is
/// built on, so a pattern written against raw characters (a glob class
/// range) can be matched against folded names: on Windows a character's
/// simple uppercase, elsewhere the lead of a normalized fold (e.g., `É` to
/// `e` + U+0301). Matching lead characters is intentionally conservative;
/// over-matching only denies more.
pub(crate) fn unfold(folded: char) -> impl Iterator<Item = char> {
    use std::collections::HashMap;
    use std::sync::OnceLock;

    static INVERSE: OnceLock<HashMap<char, Vec<char>>> = OnceLock::new();
    let inverse = INVERSE.get_or_init(|| {
        let mut inverse: HashMap<char, Vec<char>> = HashMap::new();
        for c in (0..=0x10_FFFF).filter_map(char::from_u32) {
            let mut fold = fold_char(c);
            if let Some(first) = fold.next()
                && (first != c || fold.next().is_some())
            {
                inverse.entry(first).or_default().push(c);
            }
        }
        inverse
    });
    std::iter::once(folded).chain(inverse.get(&folded).into_iter().flatten().copied())
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
    if has_short_name_shape(&raw.to_string_lossy()) {
        return Err("path component has the shape of an 8.3 short name");
    }
    Ok(())
}

/// `SETTIN~1.JSO`, `PROGRA~1`, `AB12~10.TXT`: a base of 1 to 8 characters
/// ending in `~` and digits, and an optional extension of 1 to 3. NTFS
/// generates such aliases for long names; opening one reaches the long name,
/// so whether a denied file stands behind an alias would show in the answer.
/// Refusing the shape outright answers the same whatever is on disk.
#[cfg(windows)]
fn has_short_name_shape(name: &str) -> bool {
    let (base, extension) = match name.rsplit_once('.') {
        Some((base, extension)) => (base, Some(extension)),
        None => (name, None),
    };
    if base.is_empty() || base.contains('.') || base.chars().count() > 8 {
        return false;
    }
    if extension.is_some_and(|e| e.is_empty() || e.chars().count() > 3) {
        return false;
    }
    base.rsplit_once('~')
        .is_some_and(|(_, digits)| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
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
        let nul = OsString::from("a\0b");
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
    fn windows_rejects_short_name_shapes() {
        for path in [
            r"C:\data\SETTIN~1.JSO",
            r"C:\data\progra~1",
            r"C:\data\AB12~10.TXT",
            r"C:\data\x~1.t",
            r"C:\PROGRA~1\data\x.txt",
        ] {
            assert!(invalid(path), "{path} must be rejected");
        }
        for path in [
            r"C:\data\~$doc.docx",
            r"C:\data\~WRL0001.tmp",
            r"C:\data\file.txt~",
            r"C:\data\name~1.docx",
            r"C:\data\long-name~1.txt",
            r"C:\data\a~b.txt",
            r"C:\data\a.b~1",
        ] {
            assert!(!invalid(path), "{path} is not a short-name shape");
        }
    }

    // Glob classes match folded names through unfold: a character missing
    // from it lets a name a deny class covers through, an extra one only
    // denies more. Both directions, for every character: every character is
    // among the unfold of its fold's lead, and unfold yields only characters
    // whose fold that character leads
    #[test]
    fn unfold_inverts_fold() {
        let lead = |c: char| {
            let mut buf = [0; 4];
            let folded = fold(OsStr::new(c.encode_utf8(&mut buf)));
            String::from_utf8(folded).ok()?.chars().next()
        };
        for c in (0..=0x10_FFFF).filter_map(char::from_u32) {
            if let Some(first) = lead(c) {
                assert!(
                    unfold(first).any(|x| x == c),
                    "{c:?} folds to a name led by {first:?}, whose unfold misses it"
                );
            }
            for x in unfold(c).skip(1) {
                assert_eq!(lead(x), Some(c), "unfold({c:?}) yields {x:?}");
            }
        }
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

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn keys_fold_case_normalization_and_ignorables() {
        let key = |s: &str| Key::of(OsStr::new(s));
        assert_eq!(key("Secret"), key("SECRET"));
        assert_eq!(key("caf\u{e9}"), key("cafe\u{301}"), "NFC and NFD");
        assert_eq!(
            key("CAF\u{c9}"),
            key("cafe\u{301}"),
            "case and normalization"
        );
        assert_eq!(key("sec\u{200c}ret"), key("secret"), "HFS+ ignorable");
        assert_eq!(key("se\u{ad}cret"), key("secret"), "casefold ignorable");
        assert_eq!(
            key("\u{2764}\u{fe0f}.txt"),
            key("\u{2764}.txt"),
            "variation selector"
        );
        assert_eq!(key("stra\u{df}e"), key("STRASSE"), "full case folding");
        assert_ne!(key("secret"), key("secrets"));
    }
}

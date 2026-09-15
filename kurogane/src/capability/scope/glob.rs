//! Deny globs matched against root-relative path keys.
//!
//! Matching uses literal separators and leading-dot components with `**`
//! matching zero or more components including hidden ones. Thus `*`, `?` and
//! `[...]` cannot cross a component boundary, never match a leading `.`
//! unless the pattern component starts with a literal `.` (`\.env` is one),
//! and `**/.git` can exclude `.git` from a recursive allow.
//!
//! `?` and `[...]` consume one whole character. Names are decoded strictly:
//! a byte that does not start a valid character is one unit of its own,
//! which `?`, `*` and negated classes match and no literal does.
//!
//! Case follows the platform: literals are compared through the same fold as
//! [`Key`] values (case-insensitive on Windows); class ranges keep the
//! characters the pattern names and match a folded name character through
//! every character that folds to it. Patterns are relative to each allow
//! root, use `/` as the separator on every platform and use `\` to escape the
//! next character. A pattern matching a directory denies its entire subtree
//! ([`Glob::covers`]).
//!
//! Globs only ever deny, so wherever matching cannot be exact it errs toward
//! matching more. Matching is a state-set simulation, linear in the path and
//! pattern lengths: a hostile path cannot make it backtrack.
//!
//! These matching rules preserve the behavior required to avoid the
//! traversal issue addressed by CVE-2022-46171.

use std::ffi::OsStr;

use crate::capability::path::{fold, unfold, Key};

/// The most parts (components and `**`) a pattern may have: a `u64` holds
/// one bit per part plus the accepting position.
const MAX_PARTS: usize = 63;
/// The most tokens one pattern component may have; bounds matching cost.
const MAX_TOKENS: usize = 1024;

/// Unit of a decoded name: a code point, or `INVALID + byte` for a byte that
/// does not start a valid (WTF-8) character.
const INVALID: u32 = 0x11_0000;
const DOT: u32 = '.' as u32;

/// A parsed deny glob.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Glob {
    parts: Vec<GlobPart>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum GlobPart {
    /// Zero or more components (`**`).
    AnyDepth,
    Component(ComponentGlob),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ComponentGlob {
    tokens: Vec<Token>,
    /// The pattern component starts with a literal `.`.
    literal_dot: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    /// One folded unit.
    Lit(u32),
    /// `*`: any run of units within one component.
    Any,
    /// `?`: exactly one unit.
    One,
    /// `[...]` / `[!...]`: exactly one unit.
    Class(Class),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Class {
    negated: bool,
    /// Inclusive ranges of the characters the pattern names, unfolded.
    ranges: Vec<(char, char)>,
}

/// What a glob still requires after part of a path: nothing (the path or an
/// ancestor matched, so everything beneath is denied), or the set of pattern
/// positions still live (`Live(0)`: nothing beneath can ever match).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Residual {
    Covered,
    Live(u64),
}

impl Glob {
    pub(crate) fn parse(pattern: &str) -> Result<Glob, &'static str> {
        if pattern.starts_with('/') {
            return Err("patterns are relative to the scope's allow roots");
        }
        let mut parts = Vec::new();
        for component in pattern.split('/').filter(|c| !c.is_empty()) {
            if component == "." || component == ".." {
                return Err("pattern components may not be '.' or '..'");
            }
            if component == "**" {
                // `**/**` means what `**` means
                if parts.last() != Some(&GlobPart::AnyDepth) {
                    parts.push(GlobPart::AnyDepth);
                }
            } else {
                parts.push(GlobPart::Component(parse_component(component)?));
            }
        }
        if parts.is_empty() {
            return Err("pattern is empty");
        }
        if parts.len() > MAX_PARTS {
            return Err("pattern has too many components");
        }
        Ok(Glob { parts })
    }

    /// True when the pattern matches `rel` or one of its ancestors.
    pub(crate) fn covers(&self, rel: &[Key]) -> bool {
        self.residual(rel) == Residual::Covered
    }

    /// What the pattern still requires beneath `rel`.
    pub(crate) fn residual(&self, rel: &[Key]) -> Residual {
        let mut live = self.close(1);
        for key in rel {
            if live & self.accept() != 0 {
                return Residual::Covered;
            }
            if live == 0 {
                return Residual::Live(0);
            }
            live = self.step(live, key);
        }
        if live & self.accept() != 0 {
            Residual::Covered
        } else {
            Residual::Live(live)
        }
    }

    /// The bit of the accepting position (after the last part).
    fn accept(&self) -> u64 {
        1 << self.parts.len()
    }

    /// Adds the positions reachable without consuming a component: past each
    /// `**`, which may match zero components.
    fn close(&self, mut live: u64) -> u64 {
        for (i, part) in self.parts.iter().enumerate() {
            if live & (1 << i) != 0 && *part == GlobPart::AnyDepth {
                live |= 1 << (i + 1);
            }
        }
        live
    }

    /// Consumes one component.
    fn step(&self, live: u64, key: &Key) -> u64 {
        let units = decode(key.as_bytes());
        let mut next = 0;
        for (i, part) in self.parts.iter().enumerate() {
            if live & (1 << i) == 0 {
                continue;
            }
            match part {
                GlobPart::AnyDepth => next |= 1 << i,
                GlobPart::Component(c) if c.matches(&units) => next |= 1 << (i + 1),
                GlobPart::Component(_) => {}
            }
        }
        self.close(next)
    }
}

impl ComponentGlob {
    /// Simulates the token automaton over `name`; `live[i]` means "token `i`
    /// is next", `live[len]` that the whole component matched.
    fn matches(&self, name: &[u32]) -> bool {
        if name.first() == Some(&DOT) && !self.literal_dot {
            return false;
        }
        let len = self.tokens.len();
        let mut live = vec![false; len + 1];
        let mut next = vec![false; len + 1];
        live[0] = true;
        self.close(&mut live);
        for &unit in name {
            next.fill(false);
            for (i, token) in self.tokens.iter().enumerate() {
                if !live[i] {
                    continue;
                }
                match token {
                    Token::Any => next[i] = true,
                    Token::One => next[i + 1] = true,
                    Token::Lit(lit) if *lit == unit => next[i + 1] = true,
                    Token::Class(class) if class.matches(unit) => next[i + 1] = true,
                    Token::Lit(_) | Token::Class(_) => {}
                }
            }
            // A combining mark belongs to the character before it: after `?`
            // or a class the mark may be absorbed too, so a decomposed `é`
            // (`e` + U+0301) is one character there. Only ever adds matches
            if is_mark(unit) {
                for (i, token) in self.tokens.iter().enumerate() {
                    if live[i + 1] && matches!(token, Token::One | Token::Class(_)) {
                        next[i + 1] = true;
                    }
                }
            }
            self.close(&mut next);
            if !next.contains(&true) {
                return false;
            }
            std::mem::swap(&mut live, &mut next);
        }
        live[len]
    }

    /// Adds the positions past each `*`, which may match nothing.
    fn close(&self, live: &mut [bool]) {
        for (i, token) in self.tokens.iter().enumerate() {
            if live[i] && *token == Token::Any {
                live[i + 1] = true;
            }
        }
    }
}

impl Class {
    /// Whether the folded unit `unit` may stand for a character this class
    /// admits. Over-matches where folding is ambiguous: some character that
    /// folds to `unit` is enough.
    fn matches(&self, unit: u32) -> bool {
        let Some(folded) = char::from_u32(unit) else {
            // An invalid byte is no character the pattern can name
            return self.negated;
        };
        unfold(folded).any(|c| self.contains(c) != self.negated)
    }

    fn contains(&self, c: char) -> bool {
        self.ranges.iter().any(|&(lo, hi)| lo <= c && c <= hi)
    }
}

fn parse_component(component: &str) -> Result<ComponentGlob, &'static str> {
    let mut tokens = Vec::new();
    let mut chars = component.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                let next = chars.next().ok_or("dangling '\\' at end of a component")?;
                push_literal(&mut tokens, next);
            }
            '*' => {
                if tokens.last() != Some(&Token::Any) {
                    tokens.push(Token::Any);
                }
            }
            '?' => tokens.push(Token::One),
            '[' => tokens.push(Token::Class(parse_class(&mut chars)?)),
            other => push_literal(&mut tokens, other),
        }
    }
    if tokens.len() > MAX_TOKENS {
        return Err("pattern component is too long");
    }
    if let Some(Token::Class(class)) = tokens.first()
        && !class.negated
        && class.contains('.')
    {
        return Err("a leading '[...]' never matches a leading '.'; write a literal '.'");
    }
    Ok(ComponentGlob {
        literal_dot: tokens.first() == Some(&Token::Lit(DOT)),
        tokens,
    })
}

/// Parses a class after its `[`.
fn parse_class(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
) -> Result<Class, &'static str> {
    const UNTERMINATED: &str = "unterminated character class";
    let negated = chars.next_if_eq(&'!').is_some();
    let mut ranges = Vec::new();
    loop {
        let c = chars.next().ok_or(UNTERMINATED)?;
        if c == ']' && !ranges.is_empty() {
            return Ok(Class { negated, ranges });
        }
        let lo = if c == '\\' {
            chars.next().ok_or(UNTERMINATED)?
        } else {
            c
        };
        let mut ahead = chars.clone();
        if ahead.next() == Some('-')
            && let Some(hi) = ahead.next().filter(|&hi| hi != ']')
        {
            chars.next();
            chars.next();
            let hi = if hi == '\\' {
                chars.next().ok_or(UNTERMINATED)?
            } else {
                hi
            };
            ranges.push((lo.min(hi), lo.max(hi)));
        } else {
            ranges.push((lo, lo));
        }
    }
}

/// Whether `unit` is a combining mark (a nonzero canonical combining class).
fn is_mark(unit: u32) -> bool {
    char::from_u32(unit)
        .is_some_and(|c| unicode_normalization::char::canonical_combining_class(c) != 0)
}

/// A literal character becomes the units of its fold.
fn push_literal(tokens: &mut Vec<Token>, c: char) {
    let mut buf = [0; 4];
    let folded = fold(OsStr::new(c.encode_utf8(&mut buf)));
    tokens.extend(decode(&folded).into_iter().map(Token::Lit));
}

/// Decodes a key into units: code points of a strict (WTF-8) decoding, with
/// every byte that does not start a valid character as a unit of its own.
fn decode(bytes: &[u8]) -> Vec<u32> {
    let continuation = |b: Option<&u8>| {
        b.filter(|&&b| b & 0xC0 == 0x80)
            .map(|&b| u32::from(b & 0x3F))
    };
    let mut units = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b0 = bytes[i];
        let (unit, len) = match b0 {
            0x00..=0x7F => (u32::from(b0), 1),
            0xC2..=0xDF => match continuation(bytes.get(i + 1)) {
                Some(c1) => ((u32::from(b0 & 0x1F) << 6) | c1, 2),
                None => (INVALID + u32::from(b0), 1),
            },
            0xE0..=0xEF => match (
                continuation(bytes.get(i + 1)),
                continuation(bytes.get(i + 2)),
            ) {
                (Some(c1), Some(c2)) => {
                    let cp = (u32::from(b0 & 0x0F) << 12) | (c1 << 6) | c2;
                    // Overlong forms are invalid; surrogates (WTF-8) are kept
                    if cp >= 0x800 {
                        (cp, 3)
                    } else {
                        (INVALID + u32::from(b0), 1)
                    }
                }
                _ => (INVALID + u32::from(b0), 1),
            },
            0xF0..=0xF4 => match (
                continuation(bytes.get(i + 1)),
                continuation(bytes.get(i + 2)),
                continuation(bytes.get(i + 3)),
            ) {
                (Some(c1), Some(c2), Some(c3)) => {
                    let cp = (u32::from(b0 & 0x07) << 18) | (c1 << 12) | (c2 << 6) | c3;
                    if (0x1_0000..=0x10_FFFF).contains(&cp) {
                        (cp, 4)
                    } else {
                        (INVALID + u32::from(b0), 1)
                    }
                }
                _ => (INVALID + u32::from(b0), 1),
            },
            _ => (INVALID + u32::from(b0), 1),
        };
        units.push(unit);
        i += len;
    }
    units
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(path: &str) -> Vec<Key> {
        path.split('/')
            .filter(|c| !c.is_empty())
            .map(|c| Key::of(OsStr::new(c)))
            .collect()
    }

    fn covers(pattern: &str, path: &str) -> bool {
        Glob::parse(pattern)
            .expect("valid pattern")
            .covers(&keys(path))
    }

    #[test]
    fn literal_components_respect_boundaries() {
        assert!(covers("notes/secret", "notes/secret"));
        assert!(!covers("notes/secret", "notes/secrets"));
        assert!(!covers("notes/secret", "notes"));
    }

    #[test]
    fn a_matched_directory_denies_its_subtree() {
        assert!(covers("secret*", "secret-dir/inner/file.txt"));
        assert!(covers("**/.git", "a/.git/objects/pack"));
        assert!(!covers("**/.git", "a/git/objects"));
    }

    #[test]
    fn star_does_not_cross_separators() {
        assert!(covers("*.key", "secret.key"));
        assert!(!covers("*.key", "sub/secret.key"));
        assert!(covers("**/*.key", "sub/deeper/secret.key"));
    }

    #[test]
    fn wildcards_never_match_a_leading_dot() {
        assert!(!covers("*.key", ".key"));
        assert!(!covers("?oobar", ".oobar"));
        assert!(!covers("[!a]oobar", ".oobar"));
        assert!(!covers("*", ".hidden"));
        assert!(covers(".*", ".key"));
        assert!(covers(".o*", ".oobar"));
    }

    #[test]
    fn escaped_leading_dot_matches() {
        assert!(covers(r"\.env", ".env"));
        assert!(covers(r"**/\.secrets", "a/b/.secrets/token"));
        assert!(!covers(r"\.env", "env"));
    }

    #[test]
    fn a_leading_class_naming_a_dot_is_rejected() {
        for bad in ["[.]env", "**/[.a]git", "[+-/]x"] {
            assert!(Glob::parse(bad).is_err(), "{bad:?}");
        }
        assert!(
            Glob::parse("x[.]env").is_ok(),
            "a class after the first token is fine"
        );
    }

    #[test]
    fn double_star_matches_any_depth() {
        assert!(covers("**", ""));
        assert!(covers("a/**/z", "a/z"));
        assert!(covers("a/**/z", "a/b/c/z"));
        assert!(covers("a/**", "a"));
        assert!(covers("**/**/z", "z"));
        assert!(!covers("*/x", "a/b/x"));
    }

    #[test]
    fn question_mark_and_classes() {
        assert!(covers("?.txt", "b.txt"));
        assert!(!covers("?.txt", "bb.txt"));
        assert!(covers("[ab].txt", "a.txt"));
        assert!(!covers("[ab].txt", "c.txt"));
        assert!(covers("[!ab].txt", "c.txt"));
        assert!(covers("[a-c].txt", "b.txt"));
        assert!(covers("[]].txt", "].txt"));
        assert!(covers(r"[\]x].txt", "].txt"));
    }

    #[test]
    fn classes_match_whole_characters() {
        assert!(covers("secret?.txt", "secretФ.txt"));
        assert!(!covers("secret?.txt", "secretAB.txt"));
        assert!(covers("secret[!a].txt", "secretФ.txt"));
        assert!(covers("note[äö].md", "noteä.md"));
        assert!(!covers("note[äö].md", "noteü.md"));
        assert!(covers("[а-я]*", "файл"), "a range of Cyrillic characters");
    }

    #[test]
    fn escaped_metacharacters_are_literal() {
        assert!(covers(r"a\*b", "a*b"));
        assert!(!covers(r"a\*b", "aXb"));
        assert!(covers(r"x\[1\]", "x[1]"));
    }

    #[test]
    fn malformed_patterns_are_rejected() {
        for bad in ["", "/abs/*.key", "a/../b", "foo[1", "foo\\", "[!", "a[b-"] {
            assert!(Glob::parse(bad).is_err(), "{bad:?}");
        }
        let too_many = vec!["a"; MAX_PARTS + 1].join("/");
        assert!(Glob::parse(&too_many).is_err());
        let too_long = "?".repeat(MAX_TOKENS + 1);
        assert!(Glob::parse(&too_long).is_err());
    }

    #[test]
    fn matching_does_not_backtrack() {
        // Exponential for a backtracking matcher; one pass here
        let name = "a".repeat(250);
        let deep = vec![name.as_str(); 1024].join("/");
        for pattern in [
            "*a*a*a*a*a*a*a*a*a*b",
            "**/*a*a*a*a*b/**",
            "**/**/**/x/**/y",
        ] {
            assert!(!covers(pattern, &deep), "{pattern}");
        }
    }

    #[test]
    fn residuals_report_what_remains() {
        let glob = Glob::parse("vault/*.key").unwrap();
        assert_eq!(glob.residual(&keys("vault/a.key")), Residual::Covered);
        assert!(matches!(glob.residual(&keys("vault")), Residual::Live(live) if live != 0));
        assert_eq!(glob.residual(&keys("other")), Residual::Live(0));
        let any = Glob::parse("**/*.key").unwrap();
        assert_eq!(
            any.residual(&keys("a")),
            any.residual(&keys("b/c")),
            "`**/` is location-independent"
        );
    }

    #[cfg(unix)]
    #[test]
    fn invalid_bytes_are_units_of_their_own() {
        use std::os::unix::ffi::OsStrExt;
        let key = |bytes: &[u8]| Key::of(OsStr::from_bytes(bytes));
        let glob = Glob::parse("?.key").unwrap();
        assert!(
            glob.covers(&[key(b"\xC3.key")]),
            "`?` swallowed the dot after an invalid lead byte"
        );
        assert!(Glob::parse("a?b").unwrap().covers(&[key(b"a\xE2b")]));
        assert!(
            !Glob::parse("a\u{e9}b").unwrap().covers(&[key(b"a\xC3b")]),
            "an invalid byte matched a literal"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_matching_is_case_insensitive() {
        assert!(covers("*.key", "SECRET.KEY"));
        assert!(covers("**/.GIT", "a/.git/config"));
        assert!(covers("[a-c].txt", "B.TXT"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_class_ranges_keep_their_members() {
        // `[A-z]` spans `[\]^_\``; folding the pattern first would shrink it
        assert!(covers("x[A-z].txt", "x_.txt"));
        assert!(covers("y[Z-a].txt", "y_.txt"));
        assert!(covers("secret[!a].txt", "secretB.txt"));
    }

    #[test]
    fn a_decomposed_character_is_one_character() {
        // `e` + COMBINING ACUTE ACCENT is one character for `?` and classes
        assert!(covers("caf?.txt", "cafe\u{301}.txt"));
        assert!(covers("caf[a-z].txt", "cafe\u{301}.txt"));
        assert!(
            covers("secret?.txt", "secreto\u{308}\u{301}.txt"),
            "several marks"
        );
        assert!(!covers("caf?.txt", "cafee.txt"), "still one base character");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_matching_is_case_sensitive() {
        assert!(!covers("secret", "SECRET"));
        assert!(!covers("[a-c].txt", "B.txt"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_matching_folds_case_and_normalization() {
        assert!(covers("*.key", "SECRET.KEY"));
        assert!(
            covers("caf\u{e9}.txt", "CAFE\u{301}.TXT"),
            "NFC pattern, NFD name"
        );
        assert!(
            covers("cafe\u{301}.txt", "caf\u{e9}.txt"),
            "NFD pattern, NFC name"
        );
        assert!(
            covers("note[\u{e4}].md", "note\u{c4}.md"),
            "a class across case"
        );
        assert!(
            covers("note[\u{e4}].md", "notea\u{308}.md"),
            "a class across normalization"
        );
    }
}

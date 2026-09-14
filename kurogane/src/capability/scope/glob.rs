//! Deny globs matched against root-relative path keys.
//!
//! Matching uses literal separators and leading-dot components with `**`
//! matching zero or more components including hidden ones. Thus `*` and `?`
//! cannot cross a component boundary, `*` does not match a leading `.` and
//! `**/.git` can exclude `.git` from a recursive allow.
//!
//! Case follows the platform; patterns and names are compared as [`Key`] values,
//! with case folding on Windows. Patterns are relative to each allow root,
//! use `/` as the separator on every platform and use `\` to escape the next
//! character. A pattern matching a directory denies its entire subtree
//! ([`Glob::covers`]).
//!
//! These matching rules preserve the behavior required to avoid the
//! traversal issue addressed by CVE-2022-46171.

use std::ffi::OsStr;

use crate::capability::path::{fold, Key};

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
    Lit(Vec<u8>),
    /// `*`: any run of bytes within one component.
    Any,
    /// `?`: exactly one byte.
    One,
    /// `[...]` / `[!...]`.
    Class(Class),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Class {
    negated: bool,
    /// Inclusive byte ranges; single bytes are stored as `(b, b)`.
    items: Vec<(u8, u8)>,
}

impl Glob {
    pub(crate) fn parse(pattern: &str) -> Result<Glob, &'static str> {
        if pattern.starts_with('/') {
            return Err("patterns are relative to the scope's allow roots");
        }
        let folded = fold(OsStr::new(pattern));
        let mut parts = Vec::new();
        for component in folded.split(|&b| b == b'/').filter(|c| !c.is_empty()) {
            if component == b"." || component == b".." {
                return Err("pattern components may not be '.' or '..'");
            }
            if component == b"**" {
                parts.push(GlobPart::AnyDepth);
            } else {
                parts.push(GlobPart::Component(parse_component(component)?));
            }
        }
        if parts.is_empty() {
            return Err("pattern is empty");
        }
        Ok(Glob { parts })
    }

    /// True when the pattern matches `rel` or one of its ancestors.
    pub(crate) fn covers(&self, rel: &[Key]) -> bool {
        (0..=rel.len()).any(|depth| match_parts(&self.parts, &rel[..depth]))
    }
}

fn match_parts(parts: &[GlobPart], path: &[Key]) -> bool {
    let Some((first, rest)) = parts.split_first() else {
        return path.is_empty();
    };
    match first {
        GlobPart::AnyDepth => (0..=path.len()).any(|skip| match_parts(rest, &path[skip..])),
        GlobPart::Component(c) => match path.split_first() {
            Some((name, tail)) => match_component(c, name.as_bytes()) && match_parts(rest, tail),
            None => false,
        },
    }
}

fn match_component(component: &ComponentGlob, name: &[u8]) -> bool {
    if name.first() == Some(&b'.') && !component.literal_dot {
        return false;
    }
    match_tokens(&component.tokens, name)
}

fn match_tokens(tokens: &[Token], name: &[u8]) -> bool {
    let Some((first, rest)) = tokens.split_first() else {
        return name.is_empty();
    };
    match first {
        Token::Any => (0..=name.len()).any(|take| match_tokens(rest, &name[take..])),
        Token::One => !name.is_empty() && match_tokens(rest, &name[1..]),
        Token::Lit(lit) => name.starts_with(lit) && match_tokens(rest, &name[lit.len()..]),
        Token::Class(class) => match name.split_first() {
            Some((&b, tail)) => class.matches(b) && match_tokens(rest, tail),
            None => false,
        },
    }
}

impl Class {
    fn matches(&self, b: u8) -> bool {
        let in_class = self.items.iter().any(|&(lo, hi)| lo <= b && b <= hi);
        in_class != self.negated
    }
}

fn parse_component(component: &[u8]) -> Result<ComponentGlob, &'static str> {
    let literal_dot = component.first() == Some(&b'.');
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < component.len() {
        match component[i] {
            b'\\' => {
                let &next = component
                    .get(i + 1)
                    .ok_or("dangling '\\' at end of a component")?;
                push_lit(&mut tokens, next);
                i += 2;
            }
            b'*' => {
                tokens.push(Token::Any);
                i += 1;
            }
            b'?' => {
                tokens.push(Token::One);
                i += 1;
            }
            b'[' => {
                let (class, consumed) =
                    parse_class(&component[i + 1..]).ok_or("unterminated character class")?;
                tokens.push(Token::Class(class));
                i += 1 + consumed;
            }
            other => {
                push_lit(&mut tokens, other);
                i += 1;
            }
        }
    }
    Ok(ComponentGlob {
        tokens,
        literal_dot,
    })
}

fn parse_class(rest: &[u8]) -> Option<(Class, usize)> {
    let negated = rest.first() == Some(&b'!');
    let mut idx = usize::from(negated);
    let mut items = Vec::new();
    while let Some(&b) = rest.get(idx) {
        if b == b']' && !items.is_empty() {
            return Some((Class { negated, items }, idx + 1));
        }
        if b == b'\\' {
            let &next = rest.get(idx + 1)?;
            items.push((next, next));
            idx += 2;
            continue;
        }
        idx += 1;
        match (rest.get(idx), rest.get(idx + 1)) {
            (Some(b'-'), Some(&end)) if end != b']' => {
                items.push((b, end));
                idx += 2;
            }
            _ => items.push((b, b)),
        }
    }
    None
}

fn push_lit(tokens: &mut Vec<Token>, byte: u8) {
    if let Some(Token::Lit(existing)) = tokens.last_mut() {
        existing.push(byte);
    } else {
        tokens.push(Token::Lit(vec![byte]));
    }
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
        assert!(!covers("*", ".hidden"));
        assert!(covers(".*", ".key"));
        assert!(covers(".o*", ".oobar"));
    }

    #[test]
    fn double_star_matches_any_depth() {
        assert!(covers("**", ""));
        assert!(covers("a/**/z", "a/z"));
        assert!(covers("a/**/z", "a/b/c/z"));
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
    }

    #[test]
    fn escaped_metacharacters_are_literal() {
        assert!(covers(r"a\*b", "a*b"));
        assert!(!covers(r"a\*b", "aXb"));
        assert!(covers(r"x\[1\]", "x[1]"));
    }

    #[test]
    fn malformed_patterns_are_rejected() {
        for bad in ["", "/abs/*.key", "a/../b", "foo[1", "foo\\"] {
            assert!(Glob::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_matching_is_case_insensitive() {
        assert!(covers("*.key", "SECRET.KEY"));
        assert!(covers("**/.GIT", "a/.git/config"));
    }

    #[cfg(not(windows))]
    #[test]
    fn unix_matching_is_case_sensitive() {
        assert!(!covers("secret", "SECRET"));
    }
}

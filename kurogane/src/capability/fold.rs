//! How each platform's filesystems compare names, as a value.
//!
//! [`Rules`] folds characters the way one platform's volumes compare names,
//! and inverts that fold so a glob class written against raw characters can
//! match folded names. [`Rules::NATIVE`] is this platform's, chosen at
//! compile time, so the others cost nothing at runtime; every rule set still
//! runs on every host, so each platform's fold is tested everywhere.
//!
//! Each fold identifies at least the names the platform's filesystems treat
//! as one, so folding can only make a deny rule match more, never less.

use std::collections::HashMap;
use std::sync::OnceLock;

use unicode_normalization::UnicodeNormalization;

/// One platform's way of comparing names, character by character.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Rules {
    /// Windows, as NTFS compares names: a character's simple uppercase when
    /// that is one character, else the character (`ß` stays `ß`).
    Uppercase,
    /// macOS and Linux: APFS and HFS+ are case- and normalization-insensitive
    /// by default, as are Linux casefolded directories. NFD, case lowered,
    /// raised and lowered again, default-ignorable code points dropped, NFD
    /// again. In a case-sensitive directory, names differing only in case
    /// fold alike too.
    Normalized,
    /// Elsewhere: names compare as they are.
    Exact,
}

impl Rules {
    /// This platform's rules.
    pub(crate) const NATIVE: Rules = Rules::of(std::env::consts::OS);

    /// The rules of the operating system `os`, as `std::env::consts::OS`
    /// names it.
    pub(crate) const fn of(os: &str) -> Rules {
        match os.as_bytes() {
            b"windows" => Rules::Uppercase,
            b"linux" | b"macos" => Rules::Normalized,
            _ => Rules::Exact,
        }
    }

    /// Appends the fold of `chars` to `out`. A name goes through whole: the
    /// last NFD reorders combining marks across characters.
    pub(crate) fn fold_into(self, chars: impl Iterator<Item = char>, out: &mut impl Extend<char>) {
        match self {
            Rules::Uppercase => out.extend(chars.map(|c| {
                let mut upper = c.to_uppercase();
                match (upper.next(), upper.next()) {
                    (Some(u), None) => u,
                    _ => c,
                }
            })),
            // Lowered before it is raised: `ẞ` uppercases to itself and
            // lowercases to `ß`, which only uppercasing folds to `ss`, as `ß`
            // itself folds. Every character then folds as with case mapped
            // twice over, and folding again changes nothing
            // (`the_fold_is_stable`)
            Rules::Normalized => out.extend(
                chars
                    .nfd()
                    .flat_map(char::to_lowercase)
                    .flat_map(char::to_uppercase)
                    .flat_map(char::to_lowercase)
                    .filter(|&c| !ignorable(c))
                    .nfd(),
            ),
            Rules::Exact => out.extend(chars),
        }
    }

    /// The fold inverted, built once per rule set on first use.
    pub(crate) fn inverse(self) -> &'static Inverse {
        static UPPERCASE: OnceLock<Inverse> = OnceLock::new();
        static NORMALIZED: OnceLock<Inverse> = OnceLock::new();
        static EXACT: OnceLock<Inverse> = OnceLock::new();
        let built = match self {
            Rules::Uppercase => &UPPERCASE,
            Rules::Normalized => &NORMALIZED,
            Rules::Exact => &EXACT,
        };
        built.get_or_init(|| Inverse::build(self))
    }
}

/// The most base characters one character's fold has.
pub(crate) const MAX_FOLD_STARTERS: usize = 3;

/// Whether `c` is a base character (canonical combining class 0) rather than
/// a combining mark.
pub(crate) fn is_starter(c: char) -> bool {
    unicode_normalization::char::canonical_combining_class(c) == 0
}

/// Returns `true` if `c` is a Unicode `Default_Ignorable_Code_Point` stripped
/// during filename normalization on Linux casefold (`chattr +F`) and macOS
/// (APFS/HFS+).
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

/// One rule set's fold, inverted.
pub(crate) struct Inverse {
    /// The lead of a fold with at most one base character, to the characters
    /// with that fold.
    leads: HashMap<char, Vec<char>>,
    /// The base characters of a fold with several, to the characters with
    /// that fold.
    starters: HashMap<Vec<char>, Vec<char>>,
}

impl Inverse {
    /// Yields `folded` and every character whose fold is `folded`, alone or
    /// followed by combining marks.
    ///
    /// Inverts the fold of one character: Windows' simple uppercase,
    /// elsewhere the lead of a normalized fold (e.g., `É` to `e` + U+0301).
    /// Matching the lead whatever marks follow it is conservative;
    /// over-matching only denies more. A character whose fold has several
    /// base characters is found through [`unfold_starters`](Self::unfold_starters)
    /// instead.
    pub(crate) fn unfold(&self, folded: char) -> impl Iterator<Item = char> + '_ {
        let unfolded = self.leads.get(&folded).into_iter().flatten().copied();
        std::iter::once(folded).chain(unfolded)
    }

    /// The characters whose fold has the base characters `starters`,
    /// combining marks aside: with the macOS and Linux rules `ss` for `ß`,
    /// the jamo of a Hangul syllable. Windows folds no character to several.
    pub(crate) fn unfold_starters(&self, starters: &[char]) -> &[char] {
        self.starters.get(starters).map_or(&[], Vec::as_slice)
    }

    fn build(rules: Rules) -> Inverse {
        let mut inverse = Inverse {
            leads: HashMap::new(),
            starters: HashMap::new(),
        };
        for c in (0..=0x10_FFFF).filter_map(char::from_u32) {
            let mut shape = Shape::default();
            rules.fold_into(std::iter::once(c), &mut shape);
            // An ignorable code point folds to nothing
            let Some(lead) = shape.lead else {
                continue;
            };
            if !shape.starters.is_empty() {
                inverse.starters.entry(shape.starters).or_default().push(c);
            } else if lead != c || shape.more {
                inverse.leads.entry(lead).or_default().push(c);
            }
        }
        inverse
    }
}

/// What the inverse needs of one character's fold, taken as it is folded:
/// its lead, whether more follows, and its base characters when it has
/// several (allocated only then).
#[derive(Default)]
struct Shape {
    lead: Option<char>,
    more: bool,
    starters: Vec<char>,
}

impl Extend<char> for Shape {
    fn extend<I: IntoIterator<Item = char>>(&mut self, fold: I) {
        for f in fold {
            let Some(lead) = self.lead else {
                self.lead = Some(f);
                continue;
            };
            self.more = true;
            // A fold never puts a mark before its first base character
            // (`unfold_inverts_fold`), so the lead is one
            if is_starter(f) {
                if self.starters.is_empty() {
                    self.starters.push(lead);
                }
                self.starters.push(f);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn each_platform_has_its_rules() {
        assert_eq!(Rules::of("windows"), Rules::Uppercase);
        assert_eq!(Rules::of("macos"), Rules::Normalized);
        assert_eq!(Rules::of("linux"), Rules::Normalized);
        assert_eq!(Rules::of("freebsd"), Rules::Exact);
        assert_eq!(Rules::NATIVE, Rules::of(std::env::consts::OS));
    }

    /// The fold of a name made of `c` alone, as keys fold it.
    fn fold_of(rules: Rules, c: char) -> Vec<char> {
        let mut buf = [0; 4];
        let folded = crate::capability::path::fold(rules, OsStr::new(c.encode_utf8(&mut buf)));
        String::from_utf8(folded)
            .expect("a character folds to text")
            .chars()
            .collect()
    }

    fn starters_of(fold: &[char]) -> Vec<char> {
        fold.iter().copied().filter(|&f| is_starter(f)).collect()
    }

    /// Glob classes and `?` match folded names through `unfold` and
    /// `unfold_starters`: a character missing from them lets a name a deny
    /// rule covers through, an extra one only denies more. Both directions,
    /// for every character: a fold with one base character is found by its
    /// lead alone, one with several by its base characters alone, and the
    /// tables yield nothing else.
    fn unfold_inverts_fold(rules: Rules) {
        let inverse = rules.inverse();
        for c in (0..=0x10_FFFF).filter_map(char::from_u32) {
            let fold = fold_of(rules, c);
            // An ignorable code point folds to nothing
            let Some(&lead) = fold.first() else {
                continue;
            };
            let starters = starters_of(&fold);
            assert!(
                starters.is_empty() || is_starter(lead),
                "{c:?} folds to {fold:?}, a mark before its first base character"
            );
            assert!(
                starters.len() <= MAX_FOLD_STARTERS,
                "{c:?} folds to {fold:?}"
            );
            if starters.len() > 1 {
                assert!(
                    inverse.unfold_starters(&starters).contains(&c),
                    "{c:?} folds to {fold:?}, whose base characters miss it"
                );
                assert!(
                    !inverse.unfold(lead).skip(1).any(|x| x == c),
                    "{c:?} folds to {fold:?}, and its lead alone finds it"
                );
            } else {
                assert!(
                    inverse.unfold(lead).any(|x| x == c),
                    "{c:?} folds to {fold:?}, whose lead misses it"
                );
            }
            for x in inverse.unfold(c).skip(1) {
                let fold = fold_of(rules, x);
                assert_eq!(fold.first(), Some(&c), "unfold({c:?}) yields {x:?}");
                assert!(
                    starters_of(&fold).len() <= 1,
                    "unfold({c:?}) yields {x:?}, whose fold has several base characters"
                );
            }
        }
        for (starters, chars) in &inverse.starters {
            for &x in chars {
                assert_eq!(
                    starters_of(&fold_of(rules, x)),
                    *starters,
                    "unfold_starters({starters:?}) yields {x:?}"
                );
            }
        }
    }

    #[test]
    fn unfold_inverts_the_windows_fold() {
        unfold_inverts_fold(Rules::Uppercase);
    }

    #[test]
    fn unfold_inverts_the_macos_and_linux_fold() {
        unfold_inverts_fold(Rules::Normalized);
    }

    #[test]
    fn unfold_inverts_the_exact_fold() {
        unfold_inverts_fold(Rules::Exact);
    }

    /// A filesystem folds a name to a fixed point, so a name and its fold
    /// are one name there: folding a fold must change nothing, or the two
    /// get different keys and a deny on one misses the other.
    fn the_fold_is_stable(rules: Rules) {
        for c in (0..=0x10_FFFF).filter_map(char::from_u32) {
            let once: String = fold_of(rules, c).into_iter().collect();
            let twice = crate::capability::path::fold(rules, OsStr::new(&once));
            assert_eq!(twice, once.as_bytes(), "{c:?} folds to {once:?}");
        }
    }

    #[test]
    fn the_windows_fold_is_stable() {
        the_fold_is_stable(Rules::Uppercase);
    }

    #[test]
    fn the_macos_and_linux_fold_is_stable() {
        the_fold_is_stable(Rules::Normalized);
    }
}

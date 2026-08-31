//! Official project starters and interactive selection.
//!
//! Defines named starters and provides starter and language selection.

use std::io::{self, Write};
use anyhow::{Result, bail};

use crate::tui;

/// A supported frontend language.
pub struct Language {
    pub value: &'static str,
    pub label: &'static str,
}

/// An official project starter.
pub struct Starter {
    pub name: &'static str,
    pub label: &'static str,
    pub source: &'static str,
    pub languages: &'static [Language],
}

const STARTERS: &[Starter] = &[
    Starter {
        name: "minimal",
        label: "Minimal",
        source: "https://github.com/kurogane-rs/kurogane-starter-minimal",
        languages: &[
            Language {
                value: "typescript",
                label: "TypeScript",
            },
            Language {
                value: "javascript",
                label: "JavaScript",
            },
        ],
    },
    Starter {
        name: "react",
        label: "React",
        source: "https://github.com/kurogane-rs/kurogane-starter-react",
        languages: &[
            Language {
                value: "typescript",
                label: "TypeScript",
            },
            Language {
                value: "javascript",
                label: "JavaScript",
            },
        ],
    },
    Starter {
        name: "svelte",
        label: "Svelte",
        source: "https://github.com/kurogane-rs/kurogane-starter-svelte",
        languages: &[
            Language {
                value: "typescript",
                label: "TypeScript",
            },
            Language {
                value: "javascript",
                label: "JavaScript",
            },
        ],
    },
    Starter {
        name: "vue",
        label: "Vue",
        source: "https://github.com/kurogane-rs/kurogane-starter-vue",
        languages: &[
            Language {
                value: "typescript",
                label: "TypeScript",
            },
            Language {
                value: "javascript",
                label: "JavaScript",
            },
        ],
    },
];

/// Find a starter by name.
pub fn find(name: &str) -> Option<&'static Starter> {
    STARTERS.iter().find(|s| s.name == name)
}

/// Select a starter interactively.
pub fn choose(non_interactive: bool) -> Result<&'static Starter> {
    if non_interactive {
        bail!(
            "Cannot choose a starter without a terminal.\n\
             Specify one: `kurogane new <starter> --name <NAME>`\n\
             or use a custom template: `kurogane new --template <source> --name <NAME>`."
        );
    }

    println!("? Choose a starter:\n");
    for (i, starter) in STARTERS.iter().enumerate() {
        println!("  {}) {}", i + 1, starter.label);
    }
    tui::blank();

    loop {
        print!("> ");
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let input = input.trim();

        if let Ok(index) = input.parse::<usize>()
            && index >= 1
            && index <= STARTERS.len()
        {
            return Ok(&STARTERS[index - 1]);
        }

        crate::tui::error(&format!(
            "Please enter a number between 1 and {}.",
            STARTERS.len()
        ));
    }
}

/// Resolves the starter language from the flag, or selects it interactively.
pub fn resolve_language(
    starter: &Starter,
    requested: Option<String>,
    non_interactive: bool,
) -> Result<Option<&'static str>> {
    match requested {
        Some(requested) => find_language(starter, &requested).map(Some),
        None => choose_language(starter, non_interactive),
    }
}

/// Matches a requested language against a starter's supported set.
fn find_language(starter: &Starter, requested: &str) -> Result<&'static str> {
    starter
        .languages
        .iter()
        .find(|language| language.value.eq_ignore_ascii_case(requested))
        .map(|language| language.value)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "starter '{}' does not support language '{requested}'\n\nSupported: {}",
                starter.name,
                supported_languages(starter)
            )
        })
}

fn supported_languages(starter: &Starter) -> String {
    starter
        .languages
        .iter()
        .map(|language| language.value)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Select a starter language interactively.
fn choose_language(starter: &Starter, non_interactive: bool) -> Result<Option<&'static str>> {
    if starter.languages.len() <= 1 {
        return Ok(starter.languages.first().map(|l| l.value));
    }

    if non_interactive {
        bail!(
            "Cannot choose a language without a terminal.\n\
             Specify one: `kurogane new {} --name <NAME> --language <LANGUAGE>`\n\n\
             Supported: {}",
            starter.name,
            supported_languages(starter)
        );
    }

    println!("? Choose a language:\n");
    for (i, lang) in starter.languages.iter().enumerate() {
        println!("  {}) {}", i + 1, lang.label);
    }
    tui::blank();

    loop {
        print!("> ");
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let input = input.trim();

        if let Ok(index) = input.parse::<usize>()
            && index >= 1
            && index <= starter.languages.len()
        {
            return Ok(Some(starter.languages[index - 1].value));
        }

        crate::tui::error(&format!(
            "Please enter a number between 1 and {}.",
            starter.languages.len()
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all() -> &'static [Starter] {
        STARTERS
    }

    #[test]
    fn find_resolves_registered_starter() {
        let s = find("react").unwrap();
        assert!(s.source.contains("starter-react"));
        assert_eq!(s.label, "React");
    }

    #[test]
    fn find_rejects_unknown_names() {
        assert!(find("abada").is_none());
        assert!(find("reactt").is_none());
        assert!(find("React").is_none());
        assert!(find("REACT").is_none());
    }

    #[test]
    fn a_requested_language_is_matched_case_insensitively() {
        let starter = find("react").unwrap();

        assert_eq!(find_language(starter, "TypeScript").unwrap(), "typescript");
        assert_eq!(find_language(starter, "javascript").unwrap(), "javascript");
    }

    #[test]
    fn an_unsupported_language_lists_the_supported_ones() {
        let starter = find("react").unwrap();

        let err = find_language(starter, "kotlin").unwrap_err().to_string();

        assert!(err.contains("kotlin"), "the rejected value should appear");
        assert!(err.contains("typescript"), "supported values should appear");
    }

    #[test]
    fn an_explicit_language_needs_no_terminal() {
        let starter = find("vue").unwrap();

        assert_eq!(
            resolve_language(starter, Some("javascript".into()), true).unwrap(),
            Some("javascript")
        );
    }

    #[test]
    fn non_interactive_without_a_language_names_the_flag() {
        let starter = find("svelte").unwrap();

        let err = resolve_language(starter, None, true)
            .unwrap_err()
            .to_string();

        assert!(err.contains("--language"), "got: {err}");
    }

    #[test]
    fn an_explicit_unsupported_language_fails_even_non_interactively() {
        let starter = find("react").unwrap();

        let err = resolve_language(starter, Some("kotlin".into()), true)
            .unwrap_err()
            .to_string();

        assert!(err.contains("kotlin"));
        assert!(err.contains("typescript"));
        assert!(err.contains("javascript"));
    }

    #[test]
    fn an_explicit_language_is_case_insensitive_non_interactively() {
        let starter = find("react").unwrap();

        assert_eq!(
            resolve_language(starter, Some("TyPeScRiPt".into()), true).unwrap(),
            Some("typescript")
        );
    }

    #[test]
    fn starters_support_typescript_and_javascript() {
        for starter in all() {
            assert_eq!(starter.languages.len(), 2);
            assert_eq!(starter.languages[0].value, "typescript");
            assert_eq!(starter.languages[0].label, "TypeScript");
            assert_eq!(starter.languages[1].value, "javascript");
            assert_eq!(starter.languages[1].label, "JavaScript");
        }
    }
}

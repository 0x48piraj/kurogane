//! Official project starters and interactive selection.
//!
//! Defines named starters and provides starter and language selection.

use std::io::{self, IsTerminal, Write};
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
pub fn choose() -> Result<&'static Starter> {
    if !io::stdin().is_terminal() {
        bail!(
            "Interactive starter selection requires a terminal.\n\
             Use `kurogane new <starter>` or `kurogane new --template <source>`."
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

        if let Ok(index) = input.parse::<usize>() {
            if index >= 1 && index <= STARTERS.len() {
                return Ok(&STARTERS[index - 1]);
            }
        }

        crate::tui::error(&format!(
            "Please enter a number between 1 and {}.",
            STARTERS.len()
        ));
    }
}

/// Select a starter language interactively.
pub fn choose_language(starter: &Starter) -> Result<Option<&'static str>> {
    if starter.languages.len() <= 1 {
        return Ok(starter.languages.first().map(|l| l.value));
    }

    if !io::stdin().is_terminal() {
        bail!(
            "Interactive language selection requires a terminal.\n\
             Use `kurogane new <starter>` or `kurogane new --template <source>`."
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

        if let Ok(index) = input.parse::<usize>() {
            if index >= 1 && index <= starter.languages.len() {
                return Ok(Some(starter.languages[index - 1].value));
            }
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

//! Project creation from starters and templates.
//!
//! Resolves project sources and generates new applications through
//! cargo-generate with Kurogane-specific project post-generation.

use anyhow::{Result, bail};
use std::io::{self, Write};
use std::path::Path;

use crate::starters;
use crate::template;
use crate::tui;

pub fn run(
    starter: Option<String>,
    name: Option<String>,
    language: Option<String>,
    template_src: Option<String>,
    consent: template::Consent,
) -> Result<()> {
    tui::section("Kurogane project setup");

    let (source, language) =
        resolve_source(starter, language, template_src, consent.non_interactive)?;

    let name = resolve_project_name(name, consent.non_interactive)?;

    tui::step("Creating project");
    tui::field("name", &name);

    let resolved = template::resolve(&source);
    let template_dir = template::acquire(&resolved)?;
    template::confirm_hooks(&template_dir, consent)?;

    let defines = language
        .map(|l| vec![format!("language={l}")])
        .unwrap_or_default();
    let destination = std::env::current_dir()?;
    let project =
        template::generate_project(&template_dir, &name, &destination, &defines, consent)?;

    template::write_cargo_config(&project)?;

    let generated_config = kurogane_layout::PackagingConfig::load(&project)?;

    tui::success("Project created");
    tui::field("name", &name);
    tui::blank();

    tui::info("Next steps");
    println!("    cd {}", name);

    if let Some(cmd) = &generated_config.app.frontend_install {
        println!("    {cmd}");
    }

    if let Some(cmd) = &generated_config.app.frontend_run {
        println!("    {cmd}  # start the dev server");
    }

    println!("    kurogane dev  # in another terminal");
    tui::blank();

    Ok(())
}

fn resolve_source(
    starter: Option<String>,
    language: Option<String>,
    template_src: Option<String>,
    non_interactive: bool,
) -> Result<(String, Option<String>)> {
    match (starter, template_src) {
        (None, None) => {
            let s = starters::choose(non_interactive)?;
            let lang = starters::resolve_language(s, language, non_interactive)?;
            Ok((s.source.to_owned(), lang.map(str::to_owned)))
        }
        (None, Some(tpl)) => Ok((tpl, None)),
        (Some(name), None) => {
            if name == "showcase" {
                bail!("Use 'kurogane showcase' to run the showcase demo");
            }
            let s = starters::find(&name).ok_or_else(|| {
                anyhow::anyhow!(
                    "unknown starter '{name}'\n\n\
                     Available starters:\n  minimal\n  react\n  svelte\n  vue\n\n\
                     For a custom template, use --template:\n  \
                     kurogane new --template gh:owner/repository"
                )
            })?;
            let lang = starters::resolve_language(s, language, non_interactive)?;
            Ok((s.source.to_owned(), lang.map(str::to_owned)))
        }
        (Some(_), Some(_)) => {
            bail!("--template and a starter name are mutually exclusive");
        }
    }
}

/// Resolves the project name from the flag, or prompts for it.
///
/// `--name` cannot smuggle in a name that the prompt would have rejected.
fn resolve_project_name(name: Option<String>, non_interactive: bool) -> Result<String> {
    let name = match name {
        Some(name) => name.trim().to_string(),
        None => {
            if non_interactive {
                bail!("Project name is required in non-interactive mode; pass --name <NAME>");
            }

            print!("Project name: ");
            io::stdout().flush()?;

            let mut input = String::new();
            if io::stdin().read_line(&mut input)? == 0 {
                bail!("No project name provided; input ended unexpectedly");
            }
            input.trim().to_string()
        }
    };

    validate_project_name(&name)?;

    Ok(name)
}

fn validate_project_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("Project name cannot be empty.");
    }

    if name.contains('/') || name.contains('\\') {
        bail!(
            "Project name must not contain path separators: {name:?}\n\
             Pass a bare name; use `--template` for a source path."
        );
    }

    if name.chars().any(|c| c.is_control()) {
        bail!("Project name must not contain control characters: {name:?}");
    }

    if Path::new(name).exists() {
        bail!("Directory already exists: {name}");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_name_is_rejected() {
        assert!(validate_project_name("").is_err());
    }

    #[test]
    fn existing_directory_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let existing = dir.path().join("taken");
        std::fs::create_dir(&existing).unwrap();

        assert!(validate_project_name(existing.to_str().unwrap()).is_err());
    }

    #[test]
    fn a_fresh_name_is_accepted() {
        validate_project_name("my-new-app").unwrap();
    }

    #[test]
    fn a_path_separator_is_rejected() {
        assert!(
            validate_project_name("../escape").is_err(),
            "a name must not traverse directories"
        );
        assert!(validate_project_name("a/b").is_err());
    }

    #[test]
    fn a_control_character_is_rejected() {
        assert!(validate_project_name("bad\nname").is_err());
    }

    #[test]
    fn non_interactive_without_a_name_fails_instead_of_prompting() {
        let err = resolve_project_name(None, true).unwrap_err();

        assert!(
            err.to_string().contains("--name"),
            "the error must name the flag that fixes it, got: {err}"
        );
    }

    #[test]
    fn non_interactive_accepts_an_explicit_name() {
        assert_eq!(
            resolve_project_name(Some("  my-app  ".into()), true).unwrap(),
            "my-app",
            "an explicit name is trimmed like prompted input"
        );
    }
}

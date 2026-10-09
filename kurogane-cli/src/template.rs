//! Project generation from cargo-generate templates.
//!
//! A template is a local path, a git URL or a cargo-generate shorthand such
//! as `gh:owner/repository`. A git template renders from a snapshot of its
//! default branch, refreshed on every run. Kurogane's own templates are git
//! templates too.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use cargo_generate::{GenerateArgs, TemplatePath, Vcs};

use crate::cache::templates_dir;
use crate::template_store::{self, Store};

/// Permissions for a template generation run.
#[derive(Debug, Clone, Copy, Default)]
pub struct Consent {
    /// Whether template hooks may run commands without asking.
    pub hooks: bool,

    /// Whether the run must complete without prompting.
    pub non_interactive: bool,
}

/// Inputs of one generation run.
#[derive(Debug, Clone, Default)]
pub struct Answers {
    /// Placeholder values as `name=value`.
    pub defines: Vec<String>,

    /// TOML file of placeholder values.
    pub values: Option<PathBuf>,
}

/// Generates a project at `destination/name` from the template `reference`.
pub fn generate_project(
    reference: &str,
    name: &str,
    destination: &Path,
    answers: &Answers,
    consent: Consent,
) -> Result<PathBuf> {
    generate(reference, name, destination, answers, consent, false)
}

/// Generates the template `reference` into the existing directory
/// `destination`.
pub fn generate_into_existing_dir(
    reference: &str,
    name: &str,
    destination: &Path,
    answers: &Answers,
    consent: Consent,
) -> Result<PathBuf> {
    generate(reference, name, destination, answers, consent, true)
}

/// Regenerates a project in place, preserving its build artifacts.
pub fn regenerate_project(
    reference: &str,
    name: &str,
    project_dir: &Path,
    consent: Consent,
) -> Result<PathBuf> {
    reset_project_dir(project_dir)?;
    generate_into_existing_dir(reference, name, project_dir, &Answers::default(), consent)
}

fn generate(
    reference: &str,
    name: &str,
    destination: &Path,
    answers: &Answers,
    consent: Consent,
    init: bool,
) -> Result<PathBuf> {
    let args = GenerateArgs {
        template_path: template_path(reference, consent)?,
        name: Some(name.to_owned()),
        destination: Some(destination.to_owned()),
        vcs: Some(Vcs::None),
        no_workspace: true,
        init,
        define: answers.defines.clone(),
        template_values_file: answers.values.as_deref().map(values_file).transpose()?,
        ..generation_mode(consent)
    };

    cargo_generate::generate(args).with_context(|| format!("could not generate from {reference}"))
}

/// Returns where cargo-generate reads the template `reference`, a fresh
/// snapshot for a git template.
fn template_path(reference: &str, consent: Consent) -> Result<TemplatePath> {
    let cwd = std::env::current_dir().context("failed to read the current directory")?;
    let Some(url) = template_store::git_url(reference, &cwd) else {
        return Ok(TemplatePath {
            auto_path: Some(reference.to_owned()),
            ..TemplatePath::default()
        });
    };

    let snapshot = Store::new(templates_dir()).snapshot(&url, |url, into| {
        template_store::clone(url, into, !consent.non_interactive)
    })?;
    let path = snapshot.to_str().with_context(|| {
        format!(
            "the template snapshot {} has no UTF-8 path, which cargo-generate needs",
            snapshot.display()
        )
    })?;
    Ok(TemplatePath {
        path: Some(path.to_owned()),
        ..TemplatePath::default()
    })
}

/// Returns `path` as cargo-generate takes it, refusing one that is not UTF-8
/// rather than reading another file.
fn values_file(path: &Path) -> Result<String> {
    path.to_str().map(str::to_owned).with_context(|| {
        format!(
            "the values file {} has no UTF-8 path, which cargo-generate needs",
            path.display()
        )
    })
}

/// Translates generation permissions into cargo-generate options.
fn generation_mode(consent: Consent) -> GenerateArgs {
    GenerateArgs {
        silent: consent.non_interactive,
        allow_commands: consent.hooks,
        ..GenerateArgs::default()
    }
}

/// Empties a project directory except its build directory, creating it when
/// missing.
fn reset_project_dir(project_dir: &Path) -> Result<()> {
    if !project_dir.exists() {
        return fs::create_dir_all(project_dir)
            .with_context(|| format!("failed to create directory {}", project_dir.display()));
    }

    let entries = fs::read_dir(project_dir)
        .with_context(|| format!("failed to read directory {}", project_dir.display()))?;

    for entry in entries {
        let entry = entry?;

        if entry.file_name() == "target" {
            continue;
        }

        let path = entry.path();

        if entry.file_type()?.is_dir() {
            fs::remove_dir_all(&path)
                .with_context(|| format!("failed to remove directory {}", path.display()))?;
        } else {
            fs::remove_file(&path)
                .with_context(|| format!("failed to remove {}", path.display()))?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A non-interactive run must not pre-authorize hook commands.
    #[test]
    fn ci_does_not_grant_hook_consent() {
        assert!(
            !generation_mode(Consent {
                hooks: false,
                non_interactive: true,
            })
            .allow_commands
        );
    }

    /// Non-interactive mode enables silent placeholder resolution.
    #[test]
    fn non_interactive_silences_template_placeholder_prompts() {
        assert!(
            generation_mode(Consent {
                hooks: false,
                non_interactive: true,
            })
            .silent
        );
        assert!(
            !generation_mode(Consent {
                hooks: true,
                non_interactive: false,
            })
            .silent,
            "--yes alone must not enable silent mode"
        );
    }

    #[test]
    fn regeneration_keeps_the_build_directory() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("target/debug")).unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("Cargo.toml"), "").unwrap();

        reset_project_dir(dir.path()).unwrap();

        assert!(dir.path().join("target/debug").is_dir());
        assert!(!dir.path().join("src").exists());
        assert!(!dir.path().join("Cargo.toml").exists());
    }
}

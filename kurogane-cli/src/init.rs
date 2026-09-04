//! Kurogane integration for existing projects.
//!
//! Adds the Kurogane Rust shell and project configuration to an existing
//! frontend project without modifying its existing files.

use anyhow::{bail, Result};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::template;
use crate::tui;

/// Template repository providing the Rust shell for existing projects.
pub const SHELL_TEMPLATE_REPO: &str = "https://github.com/kurogane-rs/kurogane-shell";

/// Files owned by the Kurogane integration.
const SHELL_FILES: &[&str] = &[
    "Cargo.toml",
    "src/main.rs",
    "kurogane.toml",
    ".cargo/config.toml",
];

pub fn run(
    assets: Option<PathBuf>,
    dev_url: Option<String>,
    consent: template::Consent,
) -> Result<()> {
    let dir = std::env::current_dir()?;
    initialize(&dir, SHELL_TEMPLATE_REPO, consent, assets, dev_url)
}

pub(crate) fn initialize(
    dir: &Path,
    shell_source: &str,
    consent: template::Consent,
    assets: Option<PathBuf>,
    dev_url: Option<String>,
) -> Result<()> {
    tui::section("Adding Kurogane to an existing project");

    if !dir.is_dir() {
        bail!("Directory does not exist: {}", dir.display());
    }

    if dir.join("kurogane.toml").exists() {
        bail!("This directory already contains kurogane.toml; nothing to initialize.");
    }

    let collisions: Vec<&str> = SHELL_FILES
        .iter()
        .copied()
        .filter(|f| dir.join(f).exists())
        .collect();
    if !collisions.is_empty() {
        bail!(
            "Refusing to overwrite existing files:\n{}",
            collisions
                .iter()
                .map(|f| format!("  - {f}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    let interactive = !consent.non_interactive;

    let assets = match assets {
        Some(a) => a,
        None => {
            if !interactive {
                bail!("--assets is required in non-interactive mode");
            }
            prompt_frontend_dir()?
        }
    };

    let dev_url = match dev_url {
        Some(u) => u,
        None => {
            if !interactive {
                bail!("--dev-url is required in non-interactive mode");
            }
            prompt_dev_url()?
        }
    };

    if !dir.join(&assets).exists() {
        tui::warn(&format!(
            "Frontend directory '{}' does not exist yet",
            assets.display()
        ));
        tui::info("kurogane dev will report this at runtime");
    }

    let name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow::anyhow!("Cannot derive a project name from {}", dir.display()))?
        .to_owned();

    let mut defines = Vec::new();
    // Existing frontend projects use the project root
    defines.push(format!("frontend_dist={}", assets.display()));
    defines.push(format!("dev_url={dev_url}"));

    tui::step("Integrating Kurogane");
    tui::field("project", &name);
    let source = template::resolve(shell_source);
    let template_dir = template::acquire(&source)?;
    template::confirm_hooks(&template_dir, consent)?;

    template::generate_into_existing_dir(&template_dir, &name, dir, &defines, consent)?;
    template::write_cargo_config(dir)?;

    tui::success("Kurogane added");
    tui::blank();

    tui::info("Next steps");
    println!("    kurogane dev");

    tui::blank();

    Ok(())
}

fn prompt_frontend_dir() -> Result<PathBuf> {
    print!("Frontend assets directory: ");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let input = input.trim().to_string();

    if input.is_empty() {
        bail!("Frontend assets directory is required");
    }

    Ok(PathBuf::from(input))
}

fn prompt_dev_url() -> Result<String> {
    print!("Dev server URL: ");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let input = input.trim().to_string();

    if input.is_empty() {
        bail!("Dev server URL is required");
    }

    Ok(input)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fake_web_app(dir: &Path) {
        fs::create_dir_all(dir.join("dist")).unwrap();
        fs::write(dir.join("package.json"), "{\n  \"name\": \"app\"\n}\n").unwrap();
        fs::write(dir.join("dist/index.html"), "<html></html>\n").unwrap();
    }

    /// Stand-in for the shell repository.
    fn fixture_shell(dir: &Path) {
        fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"{{crate_name}}\"\nversion = \"0.0.0\"\n\n[workspace]\n",
        )
        .unwrap();
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(
            dir.join("kurogane.toml"),
            "[app]\nname = \"{{project-name}}\"\nfrontend-dist = \"{{frontend_dist}}\"\n",
        )
        .unwrap();
        fs::write(
            dir.join("cargo-generate.toml"),
            "[placeholders.frontend_dist]\ntype = \"string\"\nprompt = \"Frontend build output directory\"\ndefault = \"dist\"\n",
        )
        .unwrap();
    }

    #[test]
    fn integration_adds_the_shell_without_touching_existing_files() {
        let app = tempfile::tempdir().unwrap();
        fake_web_app(app.path());
        let package_json_before = fs::read(app.path().join("package.json")).unwrap();

        let shell = tempfile::tempdir().unwrap();
        fixture_shell(shell.path());

        initialize(
            app.path(),
            shell.path().to_str().unwrap(),
            template::Consent::default(),
            Some(PathBuf::from("dist")),
            Some("http://localhost:5173".to_string()),
        )
        .unwrap();

        assert!(app.path().join("Cargo.toml").exists());
        assert!(app.path().join("src/main.rs").exists());
        assert!(app.path().join(".cargo/config.toml").exists());

        let manifest = fs::read_to_string(app.path().join("kurogane.toml")).unwrap();
        assert!(manifest.contains("frontend-dist = \"dist\""));

        assert_eq!(
            fs::read(app.path().join("package.json")).unwrap(),
            package_json_before,
            "pre-existing files must be untouched"
        );
    }

    #[test]
    fn missing_frontend_dir_succeeds_with_warning() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("index.html"), "<html></html>\n").unwrap();

        let shell = tempfile::tempdir().unwrap();
        fixture_shell(shell.path());

        // Should succeed even though "nonexistent" dir does not exist
        initialize(
            dir.path(),
            shell.path().to_str().unwrap(),
            template::Consent::default(),
            Some(PathBuf::from("nonexistent")),
            Some("http://localhost:5173".to_string()),
        )
        .unwrap();

        let manifest = fs::read_to_string(dir.path().join("kurogane.toml")).unwrap();
        assert!(manifest.contains("frontend-dist = \"nonexistent\""));
    }

    #[test]
    fn refuses_already_initialized_projects() {
        let dir = tempfile::tempdir().unwrap();
        fake_web_app(dir.path());
        fs::write(dir.path().join("kurogane.toml"), "[app]\n").unwrap();

        let err = initialize(
            dir.path(),
            "unused",
            template::Consent::default(),
            None,
            None,
        )
        .unwrap_err();
        assert!(err.to_string().contains("already contains kurogane.toml"));
    }

    #[test]
    fn colliding_files_abort_before_anything_is_generated() {
        let dir = tempfile::tempdir().unwrap();
        fake_web_app(dir.path());
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"theirs\"\n",
        )
        .unwrap();

        let err = initialize(
            dir.path(),
            "unused",
            template::Consent::default(),
            None,
            None,
        )
        .unwrap_err();
        assert!(err.to_string().contains("Cargo.toml"));
        assert!(
            !dir.path().join("src").exists(),
            "no generation may happen on collision"
        );
    }
}

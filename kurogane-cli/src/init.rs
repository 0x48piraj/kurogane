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

pub fn run(assume_yes: bool, assets: Option<PathBuf>, dev_url: Option<String>) -> Result<()> {
    let dir = std::env::current_dir()?;
    initialize(&dir, SHELL_TEMPLATE_REPO, assume_yes, assets, dev_url)
}

pub(crate) fn initialize(
    dir: &Path,
    shell_source: &str,
    assume_yes: bool,
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

    use std::io::IsTerminal;
    let interactive = io::stdin().is_terminal();

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
    defines.push(format!("frontend={}", assets.display()));
    defines.push(format!("dev_url={dev_url}"));

    tui::step("Integrating Kurogane");
    tui::field("project", &name);
    let source = template::resolve(shell_source);
    let template_dir = template::acquire(&source)?;
    template::confirm_hooks(&template_dir, assume_yes)?;

    template::generate_into_existing_dir(&template_dir, &name, dir, &defines)?;
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

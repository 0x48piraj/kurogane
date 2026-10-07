//! Listing of application profiles and versions.
//!
//! Prints every application's profile, the CLI's version and the CEF
//! version it was built with.

use anyhow::{Context, Result, bail};
use std::fs;
use kurogane_layout::profiles_root;

use crate::tui;

pub fn run(target: Option<String>) -> Result<()> {
    match target.as_deref() {
        Some("profiles") => list_profiles(),
        Some("version") => list_version(),
        None => list_all(),
        _ => bail!("Unknown list target. Valid targets: profiles, version"),
    }
}

/// Default: show everything
fn list_all() -> Result<()> {
    list_version()?;
    tui::blank();
    list_profiles()
}

/// Lists every Kurogane application's profile, one per application
/// identity.
fn list_profiles() -> Result<()> {
    tui::section("Kurogane Profiles");

    let profiles_dir = profiles_root();

    if !profiles_dir.exists() {
        tui::info("No profiles found");
        return Ok(());
    }

    let entries = fs::read_dir(&profiles_dir)
        .with_context(|| format!("failed to read directory {}", profiles_dir.display()))?;

    let mut names = Vec::new();

    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }

    if names.is_empty() {
        tui::info("No profiles found");
        return Ok(());
    }

    names.sort();

    for name in names {
        println!("    {name}");
    }

    Ok(())
}

/// Prints the CLI's version and the CEF version it was built with.
fn list_version() -> Result<()> {
    tui::section("Kurogane Version");

    let kurogane_version = env!("CARGO_PKG_VERSION");
    let cef_version = env!("KUROGANE_CEF_VERSION");

    tui::field("kurogane", kurogane_version);
    tui::field("cef", cef_version);

    Ok(())
}

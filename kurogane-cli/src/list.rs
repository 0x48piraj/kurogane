use anyhow::{Result, bail};
use std::fs;

use crate::tui;

pub fn run(target: Option<String>) -> Result<()> {
    match target.as_deref() {
        Some("profiles") => list_profiles(),
        Some("version") => list_version(),
        None => list_all(),
        _ => bail!("Unknown list target"),
    }
}

/// Default: show everything
fn list_all() -> Result<()> {
    list_version()?;
    println!();
    list_profiles()
}

/// Lists all cached Kurogane profiles.
fn list_profiles() -> Result<()> {
    tui::section("Kurogane Profiles");

    let base = dirs::cache_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not determine cache directory"))?
        .join("kurogane")
        .join("profiles");

    if !base.exists() {
        tui::info("No profiles found");
        return Ok(());
    }

    let mut found = false;

    for entry in fs::read_dir(&base)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }

        let name = entry.file_name();
        let name = name.to_string_lossy();

        if name.len() < 18 {
            continue;
        }

        let (left, id) = name.split_at(name.len() - 16);

        // Format: "<app>-<uid16>"
        let app = match left.strip_suffix('-') {
            Some(a) if id.chars().all(|c| c.is_ascii_hexdigit()) => a,
            _ => continue,
        };

        println!("    {:<20} {}", app, id);

        found = true;
    }

    if !found {
        tui::info("No profiles found");
    }

    Ok(())
}

/// Prints Kurogane and bundled CEF versions.
fn list_version() -> Result<()> {
    tui::section("Kurogane Version");

    let kurogane_version = env!("CARGO_PKG_VERSION");
    let cef_version = env!("KUROGANE_CEF_VERSION");

    tui::field("kurogane", kurogane_version);
    tui::field("cef", cef_version);

    Ok(())
}

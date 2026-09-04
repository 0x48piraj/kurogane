//! Listing of installed runtimes and cached profiles.
//!
//! This module provides human-readable summaries of Kurogane-managed
//! CEF versions and application runtime profiles.

use anyhow::{Result, bail};
use std::fs;
use kurogane_layout::{cache_root, PROFILE_HASH_HEX_DIGITS};

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

/// Lists all cached Kurogane profiles.
fn list_profiles() -> Result<()> {
    tui::section("Kurogane Profiles");

    let profiles_dir = cache_root().join("profiles");

    if !profiles_dir.exists() {
        tui::info("No profiles found");
        return Ok(());
    }

    let mut found = false;

    // A profile directory is "<sanitized-app>-<16-hex-digit hash>"
    let hash_width = PROFILE_HASH_HEX_DIGITS;
    let min_profile_name = hash_width + 1; // at least one app character and the "-" separator

    for entry in fs::read_dir(&profiles_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }

        let name = entry.file_name();
        let name = name.to_string_lossy();

        if name.len() < min_profile_name {
            tui::warn(&format!(
                "Skipping unrecognized cache entry (name too short): {name}"
            ));
            continue;
        }

        let (app_with_separator, id) = name.split_at(name.len() - hash_width);

        // Format: "<app>-<uid 16 hex>"
        let app = match app_with_separator.strip_suffix('-') {
            Some(a) if id.chars().all(|c| c.is_ascii_hexdigit()) => a,
            _ => {
                tui::warn(&format!(
                    "Skipping unrecognized cache entry (malformed profile): {name}"
                ));
                continue;
            }
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

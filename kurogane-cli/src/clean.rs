use anyhow::Result;
use std::fs;

use crate::tui;

pub fn run() -> Result<()> {
    tui::section("Kurogane Clean");

    let base = dirs::cache_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not determine cache directory"))?
        .join("kurogane");

    if !base.exists() {
        tui::info("Nothing to clean");
        return Ok(());
    }

    let profiles = base.join("profiles");
    let showcase = base.join("showcase");

    tui::step("Removing cache data");

    // Profiles
    if profiles.exists() {
        match fs::remove_dir_all(&profiles) {
            Ok(_) => tui::field("profiles", "removed"),
            Err(e) => {
                tui::warn(&format!("Failed to remove profiles: {}", e));
                tui::field("profiles", "failed");
            }
        }
    } else {
        tui::field("profiles", "clean");
    }

    // Showcase
    if showcase.exists() {
        match fs::remove_dir_all(&showcase) {
            Ok(_) => tui::field("showcase", "removed"),
            Err(e) => {
                tui::warn(&format!("Failed to remove showcase: {}", e));
                tui::field("showcase", "failed");
            }
        }
    } else {
        tui::field("showcase", "clean");
    }

    tui::success("Cache cleaned");

    Ok(())
}

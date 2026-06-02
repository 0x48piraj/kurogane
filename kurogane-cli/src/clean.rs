use anyhow::Result;
use std::fs;
use kurogane_layout::cache_root;

use crate::tui;

pub fn run(target: Option<String>) -> Result<()> {
    tui::section("Kurogane Clean");

    let nuclear = target.as_deref() == Some("all");

    // Confirmation
    if nuclear {
        tui::warn("This will remove ALL Kurogane data.");
        tui::warn("Including installed Chromium runtimes.");

        print!("\nContinue? [y/N]: ");
        std::io::Write::flush(&mut std::io::stdout())?;

        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;

        let confirmed = matches!(input.trim().to_lowercase().as_str(), "y" | "yes");

        println!();

        if !confirmed {
            tui::info("Aborted");
            return Ok(());
        }

        tui::step("Deprovisioning Kurogane environment");

        // Global CEF installs
        let cef = kurogane_layout::install_root();

        if cef.exists() {
            match fs::remove_dir_all(&cef) {
                Ok(_) => tui::field("cef", "removed"),
                Err(e) => {
                    tui::warn(&format!("Failed to remove CEF runtimes: {}", e));
                    tui::field("cef", "failed");
                }
            }
        } else {
            tui::field("cef", "clean");
        }
    }

    println!();

    tui::step("Cleaning build artifacts");

    // dist/
    let dist = std::path::PathBuf::from("dist");

    if dist.exists() {
        match fs::remove_dir_all(&dist) {
            Ok(_) => tui::field("dist", "removed"),
            Err(e) => {
                tui::warn(&format!("Failed to remove dist: {}", e));
                tui::field("dist", "failed");
            }
        }
    } else {
        tui::field("dist", "clean");
    }

    println!();

    // Cache
    let base = cache_root();

    if !base.exists() {
        tui::info("Nothing to clean");
        return Ok(());
    }

    let profiles = base.join("profiles");
    let showcase = base.join("showcase");

    tui::step("Clearing runtime cache");

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

    println!();

    if nuclear {
        tui::success("System-wide cleanup complete");
    } else {
        tui::success("Project cleanup complete");
    }

    Ok(())
}

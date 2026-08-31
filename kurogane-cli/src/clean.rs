//! Project and runtime cache cleanup.
//!
//! This module removes generated build artifacts and, when explicitly
//! requested, system-wide Kurogane runtime and profile data.

use anyhow::Result;
use std::fs;
use kurogane_layout::cache_root;

use crate::tui;

pub fn run(target: Option<String>, confirmed: bool, non_interactive: bool) -> Result<()> {
    tui::section("Kurogane Clean");

    let nuclear = target.as_deref() == Some("all");

    // Track failed removals for the final error
    let mut failed: Vec<&str> = Vec::new();

    // Confirm destructive system-wide cleanup
    if nuclear && !confirmed {
        tui::warn("This will remove ALL Kurogane data.");
        tui::warn("Including installed Chromium runtimes.");

        // Never prompt when running unattended
        if non_interactive {
            anyhow::bail!(
                "`clean all` needs confirmation and cannot prompt here.\n\n  \
                 Re-run with --yes to confirm."
            );
        }

        let accepted = loop {
            print!("\nContinue? [y/N]: ");
            std::io::Write::flush(&mut std::io::stdout())?;

            let mut input = String::new();
            std::io::stdin().read_line(&mut input)?;

            match input.trim() {
                "y" | "Y" | "yes" | "Yes" | "YES" => break true,
                "n" | "N" | "no" | "No" | "NO" | "" => break false,
                _ => {
                    tui::warn("Please enter y or n");
                    continue;
                }
            }
        };

        tui::blank();

        if !accepted {
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
                    failed.push("cef");
                }
            }
        } else {
            tui::field("cef", "clean");
        }

        // Project-local materialized CEF runtimes
        let target_kurogane = std::path::PathBuf::from("target").join("kurogane");

        if target_kurogane.exists() {
            match fs::remove_dir_all(&target_kurogane) {
                Ok(_) => tui::field("target/kurogane", "removed"),
                Err(e) => {
                    tui::warn(&format!("Failed to remove materialized runtimes: {}", e));
                    tui::field("target/kurogane", "failed");
                    failed.push("target/kurogane");
                }
            }
        } else {
            tui::field("target/kurogane", "clean");
        }

        // Build tools cache
        let tools = cache_root().join("tools");

        if tools.exists() {
            match fs::remove_dir_all(&tools) {
                Ok(_) => tui::field("tools", "removed"),
                Err(e) => {
                    tui::warn(&format!("Failed to remove build tools: {}", e));
                    tui::field("tools", "failed");
                    failed.push("tools");
                }
            }
        } else {
            tui::field("tools", "clean");
        }
    }

    tui::blank();

    tui::step("Cleaning build artifacts");

    // dist/
    let dist = std::path::PathBuf::from("dist");

    if dist.exists() {
        match fs::remove_dir_all(&dist) {
            Ok(_) => tui::field("dist", "removed"),
            Err(e) => {
                tui::warn(&format!("Failed to remove dist: {}", e));
                tui::field("dist", "failed");
                failed.push("dist");
            }
        }
    } else {
        tui::field("dist", "clean");
    }

    tui::blank();

    // Cache
    let base = cache_root();

    if !base.exists() {
        tui::info("Nothing to clean");

        // Report failures from earlier cleanup stages
        if !failed.is_empty() {
            anyhow::bail!(
                "Cleanup incomplete; could not remove: {}",
                failed.join(", ")
            );
        }

        return Ok(());
    }

    let profiles = base.join("profiles");
    let showcase = base.join("showcase");
    let templates = crate::cache::templates_root().ok();

    tui::step("Clearing runtime cache");

    // Templates
    if let Some(templates) = templates.filter(|p| p.exists()) {
        match fs::remove_dir_all(&templates) {
            Ok(_) => tui::field("templates", "removed"),
            Err(e) => {
                tui::warn(&format!("Failed to remove template cache: {}", e));
                tui::field("templates", "failed");
                failed.push("templates");
            }
        }
    } else {
        tui::field("templates", "clean");
    }

    // Profiles
    if profiles.exists() {
        match fs::remove_dir_all(&profiles) {
            Ok(_) => tui::field("profiles", "removed"),
            Err(e) => {
                tui::warn(&format!("Failed to remove profiles: {}", e));
                tui::field("profiles", "failed");
                failed.push("profiles");
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
                failed.push("showcase");
            }
        }
    } else {
        tui::field("showcase", "clean");
    }

    tui::blank();

    if !failed.is_empty() {
        anyhow::bail!(
            "Cleanup incomplete; could not remove: {}",
            failed.join(", ")
        );
    }

    if nuclear {
        tui::success("System-wide cleanup complete");
    } else {
        tui::success("Project cleanup complete");
    }

    Ok(())
}

//! Project and runtime cache cleanup.
//!
//! Regular cleanup removes generated project artifacts. `clean all` also
//! removes installed CEF runtimes, build caches and Kurogane application
//! profiles.

use anyhow::Result;
use std::fs;
use std::io;
use std::path::Path;
use kurogane_layout::cache_root;

use crate::tui;

pub fn run(target: Option<String>, confirmed: bool, non_interactive: bool) -> Result<()> {
    tui::section("Kurogane Clean");

    let nuclear = target.as_deref() == Some("all");

    // Track failed removals for the final error
    let mut failed: Vec<&str> = Vec::new();

    // Paths from the project metadata
    let project = cargo_metadata::MetadataCommand::new().no_deps().exec();

    // Confirm destructive system-wide cleanup
    if nuclear && !confirmed {
        tui::warn("This will remove ALL Kurogane data.");
        tui::warn("Including installed Chromium runtimes.");
        tui::warn("Including every Kurogane application's browser profile (cookies, storage).");

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
    }

    // Confirmed, at the prompt or with --yes
    if nuclear {
        tui::step("Deprovisioning Kurogane environment");

        // Global CEF installs
        let cef = kurogane_layout::install_root();
        remove("cef", "CEF runtimes", &cef, &mut failed);

        // Kurogane's build output and materialized CEF runtimes
        match &project {
            Ok(metadata) => {
                let target = crate::launch::target_dir_in(metadata.target_directory.as_std_path());
                remove(
                    "target/kurogane",
                    "Kurogane build output",
                    &target,
                    &mut failed,
                );
            }
            Err(e) => tui::field("target/kurogane", format!("skipped: {e}")),
        }

        // Shared CEF wrapper builds, keyed to the runtimes removed above
        let wrapper = cache_root().join("wrapper");
        remove("wrapper", "CEF wrapper cache", &wrapper, &mut failed);

        // Build tools cache
        let tools = cache_root().join("tools");
        remove("tools", "build tools", &tools, &mut failed);

        // Every Kurogane application's browser profiles
        let profiles = cache_root().join("profiles");
        remove("profiles", "browser profiles", &profiles, &mut failed);
    }

    tui::blank();

    tui::step("Cleaning build artifacts");

    // dist/
    match &project {
        Ok(metadata) => {
            let dist = metadata.workspace_root.as_std_path().join("dist");
            remove("dist", "dist", &dist, &mut failed);
        }
        Err(e) => tui::field("dist", format!("skipped: {e}")),
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

    let showcase = base.join("showcase");
    let templates = crate::cache::templates_root().ok();

    tui::step("Clearing runtime cache");

    // Templates
    match templates {
        Some(templates) => remove("templates", "template cache", &templates, &mut failed),
        None => tui::field("templates", "clean"),
    }

    remove("showcase", "showcase", &showcase, &mut failed);

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

/// Removes the directory `path` and reports it under `label`, recording a
/// failure in `failed`. An absent directory is already clean.
fn remove(label: &'static str, what: &str, path: &Path, failed: &mut Vec<&'static str>) {
    match fs::remove_dir_all(path) {
        Ok(()) => tui::field(label, "removed"),
        Err(e) if e.kind() == io::ErrorKind::NotFound => tui::field(label, "clean"),
        Err(e) => {
            tui::warn(&format!("Failed to remove {what}: {e}"));
            tui::field(label, "failed");
            failed.push(label);
        }
    }
}

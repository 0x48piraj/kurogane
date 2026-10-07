//! Removes generated project artifacts and Kurogane's caches.
//!
//! `clean all` also removes installed CEF runtimes, build caches and
//! application profiles.

use anyhow::Result;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use kurogane_layout::{cache_root, install_root, profiles_root};

use crate::tui;

/// A directory owned by Kurogane.
pub(crate) struct Data {
    pub(crate) label: &'static str,
    pub(crate) what: &'static str,
    pub(crate) path: PathBuf,
}

/// Returns the CEF runtimes and their build caches.
pub(crate) fn runtimes() -> [Data; 2] {
    [
        Data {
            label: "cef",
            what: "CEF runtimes",
            path: install_root(),
        },
        Data {
            label: "tools",
            what: "build tools",
            path: cache_root().join("tools"),
        },
    ]
}

/// Returns the caches managed by Kurogane's cleanup commands.
pub(crate) fn caches() -> Vec<Data> {
    let templates = crate::cache::templates_root().ok().map(|path| Data {
        label: "templates",
        what: "template cache",
        path,
    });

    let showcase = Data {
        label: "showcase",
        what: "showcase",
        path: cache_root().join("showcase"),
    };

    templates.into_iter().chain([showcase]).collect()
}

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

        let accepted = tui::confirm("Continue?")?;

        tui::blank();

        if !accepted {
            tui::info("Aborted");
            return Ok(());
        }
    }

    // Confirmed, at the prompt or with --yes
    if nuclear {
        tui::step("Deprovisioning Kurogane environment");

        // Global CEF runtimes and the build caches keyed to them
        for data in runtimes() {
            remove(data.label, data.what, &data.path, &mut failed);
        }

        // Kurogane's own files under the target directory, the CEF runtimes
        // materialized for bundles
        match &project {
            Ok(metadata) => {
                let own = crate::launch::kurogane_dir_in(metadata.target_directory.as_std_path());
                remove(
                    "target/kurogane",
                    "materialized CEF runtimes",
                    &own,
                    &mut failed,
                );
            }
            Err(e) => tui::field("target/kurogane", format!("skipped: {e}")),
        }

        // Every application profile
        remove(
            "profiles",
            "browser profiles",
            &profiles_root(),
            &mut failed,
        );
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

    tui::step("Clearing runtime cache");

    for data in caches() {
        remove(data.label, data.what, &data.path, &mut failed);
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

/// Removes the directory `path` and reports the result under `label`.
/// An absent directory is already clean.
pub(crate) fn remove(label: &'static str, what: &str, path: &Path, failed: &mut Vec<&'static str>) {
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

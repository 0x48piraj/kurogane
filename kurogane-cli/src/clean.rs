//! Removes generated project artifacts and Kurogane's caches.
//!
//! `clean all` also removes tetsu's shared CEF installation, build tools
//! and application profiles.

use anyhow::Result;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use kurogane_layout::profiles_root;

use crate::cache::{cache_root, showcase_dir, templates_dir, tools_dir};

use crate::tui;

/// A directory owned by Kurogane.
pub(crate) struct Data {
    pub(crate) label: &'static str,
    pub(crate) what: &'static str,
    pub(crate) path: PathBuf,
}

/// Returns tetsu's shared CEF installation and Kurogane's build tools.
pub(crate) fn runtimes() -> Vec<Data> {
    let cef = tetsu_download::cef_install_root().map(|path| Data {
        label: "cef",
        what: "tetsu's shared CEF installation",
        path,
    });

    let tools = Data {
        label: "tools",
        what: "build tools",
        path: tools_dir(),
    };

    cef.into_iter().chain([tools]).collect()
}

/// Returns the caches managed by Kurogane's cleanup commands.
pub(crate) fn caches() -> Vec<Data> {
    vec![
        Data {
            label: "templates",
            what: "template snapshots",
            path: templates_dir(),
        },
        Data {
            label: "showcase",
            what: "showcase",
            path: showcase_dir(),
        },
    ]
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
        tui::warn("Including tetsu's shared CEF installation, which other tetsu projects use too.");
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

        // tetsu's shared CEF installation and Kurogane's build tools
        for data in runtimes() {
            remove(data.label, data.what, &data.path, &mut failed);
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

    tui::step("Clearing caches");

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

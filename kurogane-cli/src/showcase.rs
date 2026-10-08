//! Runs Kurogane's showcase application.

use anyhow::{Result, bail};
use std::process::Command;

use crate::cache::showcase_dir;
use crate::launch::describe_status;
use crate::template;
use crate::tui;

/// The showcase template repository.
const SHOWCASE_TEMPLATE_REPO: &str = "https://github.com/kurogane-rs/kurogane-showcase";

pub fn run(consent: template::Consent) -> Result<()> {
    tui::section("Kurogane Showcase");

    let root = showcase_dir();

    tui::step("Preparing showcase environment");
    tui::field("path", root.to_string_lossy());

    // Regenerate the showcase, keeping its build directory
    tui::field("template", SHOWCASE_TEMPLATE_REPO);
    template::regenerate_project(SHOWCASE_TEMPLATE_REPO, "showcase", &root, consent)?;

    tui::step("Launching showcase...");

    let exe = std::env::current_exe()?;

    let status = Command::new(exe).arg("dev").current_dir(root).status()?;

    if !status.success() {
        bail!("Showcase failed (exit code: {})", describe_status(&status));
    }

    Ok(())
}

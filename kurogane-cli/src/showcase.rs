use std::fs;
use anyhow::{Result, bail};
use std::process::Command;
use crate::tui;
use crate::templates::extract_template;

pub fn run() -> Result<()> {
    tui::section("Kurogane Showcase");

    let root = dirs::cache_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not determine cache directory"))?
        .join("kurogane")
        .join("showcase");

    tui::step("Preparing showcase environment");
    tui::field("path", root.to_string_lossy());

    // Extract showcase template from embedded assets
    fs::create_dir_all(&root)?;
    extract_template("showcase", &root)?;

    tui::step("Launching showcase...");

    let exe = std::env::current_exe()?;

    let status = Command::new(exe)
        .arg("dev")
        .current_dir(root)
        .status()?;

    if !status.success() {
        bail!("Showcase failed");
    }

    Ok(())
}

//! Plain application build command.
//!
//! This module invokes Cargo to produce the application's release
//! binary without performing distribution packaging.

use anyhow::Result;
use std::process::Command;

use crate::tui;

pub fn run() -> Result<()> {
    tui::section("Kurogane Build");

    tui::step("Building release app...");

    let status = Command::new("cargo")
        .arg("build")
        .arg("--release")
        .status()?;

    if !status.success() {
        anyhow::bail!("Build failed");
    }

    println!();
    tui::success("Build complete");

    Ok(())
}

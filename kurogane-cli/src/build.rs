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
        let code = status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "signal".into());
        anyhow::bail!("Build failed (exit code: {code})");
    }

    tui::blank();
    tui::success("Build complete");

    Ok(())
}

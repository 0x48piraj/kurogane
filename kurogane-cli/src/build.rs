//! Plain application build command.
//!
//! This module invokes Cargo to produce the application's release
//! binary without performing distribution packaging.

use anyhow::Result;

use crate::launch;
use crate::tui;

pub fn run() -> Result<()> {
    tui::section("Kurogane Build");

    let cef = launch::ensure_cef_runtime()?;

    tui::step("Building release app...");

    let status = launch::cargo_command(&cef, "build")?
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

//! Kurogane development workflow.
//!
//! Launches the app in debug mode with the runtime configured.
//!
//! For Cargo argument passthrough, use [`crate::run`].

use anyhow::Result;

use crate::launch;
use crate::tui;

pub fn run() -> Result<()> {
    tui::section("Kurogane Dev");

    let cef = launch::ensure_cef_runtime()?;
    let status = launch::cargo_run(&cef, &[])?;

    launch::exit_with(status)
}

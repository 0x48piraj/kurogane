//! Kurogane development workflow.
//!
//! Launches the app in debug mode once the CEF it loads is installed.
//!
//! For Cargo argument passthrough, use [`crate::run`].

use anyhow::Result;

use crate::launch;
use crate::tui;

pub fn run() -> Result<()> {
    tui::section("Kurogane Dev");

    let metadata = cargo_metadata::MetadataCommand::new().exec()?;
    let version = crate::install::cef_version_of(&metadata);

    let cef = crate::install::ensure_cef_runtime(&version)?;
    let status = launch::run_app(&metadata, &cef, &[])?;

    launch::exit_with(status)
}

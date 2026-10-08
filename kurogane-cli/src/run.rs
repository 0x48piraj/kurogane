//! Runs the application through Cargo for `dev` and `run`.
//!
//! Forwards arguments to `cargo run` after installing the CEF the project's
//! application loads when it is missing. Cargo owns the argument surface;
//! Kurogane owns the runtime's installation.
//!
//! `--help` is passed to Cargo. Use `kurogane help run` for Kurogane's help.
//!
//! Exits with the application's exit code; see [`crate::launch`].

use anyhow::Result;
use std::ffi::OsString;

use crate::launch;
use crate::tui;

/// Runs the application under `title`; `kurogane dev` passes no arguments.
pub fn run(title: &str, cargo_args: Vec<OsString>) -> Result<()> {
    tui::section(title);

    if !cargo_args.is_empty() {
        tui::field("cargo", launch::describe_args(&cargo_args));
    }

    let metadata = cargo_metadata::MetadataCommand::new().exec()?;
    let version = crate::install::cef_version_of(&metadata);

    let cef = crate::install::ensure_cef_runtime(&version)?;
    let status = launch::run_app(&metadata, cef.root(), &cargo_args)?;

    launch::exit_with(status)
}

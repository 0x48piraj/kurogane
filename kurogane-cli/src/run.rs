//! Cargo passthrough.
//!
//! Forwards arguments to `cargo run` and prepares the Kurogane runtime
//! environment. Cargo owns the argument surface; Kurogane owns runtime setup.
//!
//! `--help` is passed to Cargo. Use `kurogane help run` for Kurogane's help.
//!
//! Exits with the application's exit code; see [`crate::launch`].

use anyhow::Result;
use std::ffi::OsString;

use crate::launch;
use crate::tui;

pub fn run(cargo_args: Vec<OsString>) -> Result<()> {
    tui::section("Kurogane Run");

    if !cargo_args.is_empty() {
        tui::field("cargo", launch::describe_args(&cargo_args));
    }

    let cef = launch::ensure_cef_runtime()?;
    let status = launch::cargo_run(&cef, &cargo_args)?;

    launch::exit_with(status)
}

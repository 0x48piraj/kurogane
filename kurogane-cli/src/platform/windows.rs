//! Windows-only development helpers.

use std::path::Path;
use std::process::Command;

/// Prepends the CEF runtime directory to `PATH` for DLL resolution.
pub(crate) fn set_env(cmd: &mut Command, cef: &Path) {
    let existing = std::env::var("PATH").unwrap_or_default();
    cmd.env("PATH", super::prepend_search_path(cef, &existing, ';'));
}

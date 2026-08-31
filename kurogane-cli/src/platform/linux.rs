//! Linux-only development helpers.

use std::path::Path;
use std::process::Command;

/// Prepends the CEF runtime directory to the dynamic library search path.
pub(crate) fn set_env(cmd: &mut Command, cef: &Path) {
    let existing = std::env::var("LD_LIBRARY_PATH").unwrap_or_default();
    cmd.env(
        "LD_LIBRARY_PATH",
        super::prepend_search_path(cef, &existing, ':'),
    );
}

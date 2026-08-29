//! Windows-only development helpers.

use std::path::Path;
use std::process::Command;

/// Prepends the CEF runtime directory to `PATH` for DLL resolution.
pub(crate) fn set_env(cmd: &mut Command, cef: &Path) {
    let path = std::env::var("PATH").unwrap_or_default();
    let path = if path.is_empty() {
        cef.display().to_string()
    } else {
        format!("{};{}", cef.display(), path)
    };
    cmd.env("PATH", path);
}

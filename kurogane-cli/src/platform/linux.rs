//! Linux-only development helpers.

use std::path::Path;
use std::process::Command;

/// Prepends the CEF runtime directory to the dynamic library search path.
pub(crate) fn set_env(cmd: &mut Command, cef: &Path) {
    let ld = std::env::var("LD_LIBRARY_PATH").unwrap_or_default();
    let ld = if ld.is_empty() {
        cef.display().to_string()
    } else {
        format!("{}:{}", cef.display(), ld)
    };
    cmd.env("LD_LIBRARY_PATH", ld);
}

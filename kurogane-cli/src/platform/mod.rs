//! Platform-specific development helpers for the Kurogane CLI.
//!
//! Each platform configures the runtime search path for `cargo run`.

use anyhow::Result;
use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "macos")]
mod macos;

/// Prepends a directory to a platform search path.
pub(crate) fn prepend_search_path(entry: &Path, existing: &str, separator: char) -> String {
    if existing.is_empty() {
        entry.display().to_string()
    } else {
        format!("{}{separator}{existing}", entry.display())
    }
}

/// Configures a `cargo run` process to discover the CEF runtime.
pub(crate) fn configure_runtime_env(
    cmd: &mut Command,
    cef: &Path,
    cargo_args: &[OsString],
) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        let _ = (cargo_args,);
        linux::set_env(cmd, cef);
    }
    #[cfg(target_os = "windows")]
    {
        let _ = (cargo_args,);
        windows::set_env(cmd, cef);
    }
    #[cfg(target_os = "macos")]
    {
        macos::set_env(cmd, cef, cargo_args)?;
    }
    Ok(())
}

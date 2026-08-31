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

#[cfg(any(target_os = "macos", test))]
mod probe;

/// Prepends a directory to a platform search path.
pub(crate) fn prepend_search_path(entry: &Path, existing: &str, separator: char) -> String {
    if existing.is_empty() {
        entry.display().to_string()
    } else {
        format!("{}{separator}{existing}", entry.display())
    }
}

/// Configures a `cargo run` process to discover the CEF runtime.
pub(crate) fn configure_runtime_env(cmd: &mut Command, cef: &Path) -> Result<()> {
    #[cfg(target_os = "linux")]
    linux::set_env(cmd, cef);
    #[cfg(target_os = "windows")]
    windows::set_env(cmd, cef);
    #[cfg(target_os = "macos")]
    macos::set_env(cmd, cef);

    Ok(())
}

/// Places the GPU libraries a launched application needs beside its executable.
pub(crate) fn prepare_gpu_libraries(cef: &Path, cargo_args: &[OsString]) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        macos::link_gpu_libraries(cef, cargo_args)?;
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (cef, cargo_args);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn empty_existing_value_yields_no_separator() {
        let path = prepend_search_path(&PathBuf::from("/opt/cef"), "", ':');

        assert_eq!(path, "/opt/cef");
        assert!(
            !path.ends_with(':'),
            "a trailing separator is an empty search-path entry"
        );
    }

    #[test]
    fn existing_value_is_preserved_after_the_new_entry() {
        assert_eq!(
            prepend_search_path(&PathBuf::from("/opt/cef"), "/usr/lib:/lib", ':'),
            "/opt/cef:/usr/lib:/lib"
        );
    }

    #[test]
    fn windows_uses_its_own_separator() {
        assert_eq!(
            prepend_search_path(&PathBuf::from(r"C:\cef"), r"C:\Windows", ';'),
            r"C:\cef;C:\Windows"
        );
    }
}

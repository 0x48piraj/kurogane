//! Platform-specific development helpers for the Kurogane CLI.
//!
//! Each platform configures the runtime search path for `cargo run`.

use anyhow::Result;
use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

#[cfg(target_os = "linux")]
use anyhow::Context;

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

/// Cargo build-script override that suppresses `cef-dll-sys`'s runtime staging.
///
/// Linux only; `cef-dll-sys` only emits link metadata on Linux, which this
/// override reproduces. Windows and macOS compile `libcef_dll_wrapper`; a
/// global override would suppress that build and break linking.
///
/// This workaround avoids redundant staging for read-only `CEF_PATH` trees.
#[cfg(target_os = "linux")]
pub(crate) fn cef_build_script_override(cef: &Path) -> Result<Vec<OsString>> {
    // Overrides are keyed by an exact target triple; `cfg(...)` is not accepted
    let triple = host_triple()?;
    let root = cef.display();

    Ok([
        format!(r#"target.{triple}.cef_dll_wrapper.rustc-link-search=["native={root}"]"#),
        format!(r#"target.{triple}.cef_dll_wrapper.rustc-link-lib=["cef"]"#),
        format!(r#"target.{triple}.cef_dll_wrapper.CEF_DIR="{root}""#),
    ]
    .into_iter()
    .flat_map(|entry| [OsString::from("--config"), OsString::from(entry)])
    .collect())
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn cef_build_script_override(cef: &Path) -> Result<Vec<OsString>> {
    let _ = cef;
    Ok(Vec::new())
}

/// The triple Cargo builds for by default, as reported by the active toolchain.
#[cfg(target_os = "linux")]
fn host_triple() -> Result<String> {
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| OsString::from("rustc"));

    let output = Command::new(&rustc)
        .arg("-vV")
        .output()
        .with_context(|| format!("failed to run {:?} -vV", rustc))?;

    if !output.status.success() {
        anyhow::bail!("{rustc:?} -vV exited with {}", output.status);
    }

    String::from_utf8(output.stdout)
        .context("rustc -vV produced non-UTF-8 output")?
        .lines()
        .find_map(|line| line.strip_prefix("host: ").map(str::to_owned))
        .context("rustc -vV did not report a host triple")
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

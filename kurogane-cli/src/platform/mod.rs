//! Platform-specific development helpers for the Kurogane CLI.
//!
//! Each platform configures the runtime search path for `cargo run`.

use anyhow::Result;
use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

#[cfg(any(target_os = "linux", target_os = "windows"))]
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
/// On Linux / Windows the build script resolves CEF, copies its runtime next to
/// the binaries (unused: Kurogane loads CEF from its own root) and emits link
/// directives which the override reproduces. On Windows it also compiles
/// `libcef_dll_wrapper` which nothing links: the bindings only call libcef's
/// C API and `libcef.dll` exports all of it.
///
/// macOS keeps the build script; every CEF entry point resolves through the
/// wrapper's library loader.
#[cfg(any(target_os = "linux", target_os = "windows"))]
pub(crate) fn cef_build_script_override(cef: &Path) -> Result<Vec<OsString>> {
    // Overrides require an exact target triple
    let triple = host_triple()?;

    // The override requires UTF-8 paths; assumes MSVC's `libcef.lib` on Windows
    let Some(root) = cef.to_str() else {
        return Ok(Vec::new());
    };
    if cfg!(windows) && !triple.ends_with("-msvc") {
        return Ok(Vec::new());
    }

    Ok(override_config(&triple, root)
        .into_iter()
        .flat_map(|entry| [OsString::from("--config"), OsString::from(entry)])
        .collect())
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub(crate) fn cef_build_script_override(cef: &Path) -> Result<Vec<OsString>> {
    let _ = cef;
    Ok(Vec::new())
}

/// Builds Cargo configuration overrides for the CEF wrapper.
#[cfg(any(target_os = "linux", target_os = "windows", test))]
fn override_config(triple: &str, root: &str) -> Vec<String> {
    // Windows uses the `libcef` import library; other platforms use `cef`
    let lib = if triple.contains("-windows-") {
        "libcef"
    } else {
        "cef"
    };

    // Serialize the values as TOML to preserve platform-specific path syntax
    let search = toml::Value::Array(vec![format!("native={root}").into()]);
    let link = toml::Value::Array(vec![lib.into()]);
    let dir = toml::Value::from(root);

    let key = format!("target.{triple}.cef_dll_wrapper");
    vec![
        format!("{key}.rustc-link-search={search}"),
        format!("{key}.rustc-link-lib={link}"),
        format!("{key}.CEF_DIR={dir}"),
    ]
}

/// The triple Cargo builds for by default, as reported by the active toolchain.
#[cfg(any(target_os = "linux", target_os = "windows"))]
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

    /// Extracts a field from a Cargo target override
    fn override_field(entry: &str, triple: &str, field: &str) -> toml::Value {
        let config: toml::Table = entry.parse().expect("each entry is one TOML key/value");
        config["target"][triple]["cef_dll_wrapper"][field].clone()
    }

    #[test]
    fn linux_override_links_libcef_so_from_the_root() {
        let triple = "x86_64-unknown-linux-gnu";
        let entries = override_config(triple, "/opt/cef");

        assert_eq!(
            override_field(&entries[0], triple, "rustc-link-search"),
            toml::Value::Array(vec!["native=/opt/cef".into()])
        );
        assert_eq!(
            override_field(&entries[1], triple, "rustc-link-lib"),
            toml::Value::Array(vec!["cef".into()])
        );
        assert_eq!(
            override_field(&entries[2], triple, "CEF_DIR"),
            toml::Value::from("/opt/cef")
        );
    }

    #[test]
    fn windows_override_links_the_import_library() {
        let triple = "x86_64-pc-windows-msvc";
        let root = r"C:\Users\me\AppData\Local\kurogane\cef\150.0.10";
        let entries = override_config(triple, root);

        assert_eq!(
            override_field(&entries[1], triple, "rustc-link-lib"),
            toml::Value::Array(vec!["libcef".into()]),
            "MSVC links libcef.lib, the import library of libcef.dll"
        );
        assert_eq!(
            override_field(&entries[0], triple, "rustc-link-search"),
            toml::Value::Array(vec![format!("native={root}").into()]),
            "backslashes are TOML escapes and must survive as path separators"
        );
        assert_eq!(
            override_field(&entries[2], triple, "CEF_DIR"),
            toml::Value::from(root)
        );
    }

    #[test]
    fn override_paths_survive_quotes() {
        let triple = "aarch64-unknown-linux-gnu";
        let root = r#"/home/o"neil/cef's"#;

        assert_eq!(
            override_field(&override_config(triple, root)[2], triple, "CEF_DIR"),
            toml::Value::from(root)
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

//! Locate Cargo-built executables for macOS GPU setup.
//!
//! Cargo reports the executable path directly, avoiding
//! assumptions about target and profile directories.

use std::path::{Path, PathBuf};

#[cfg(target_os = "macos")]
use anyhow::Result;
#[cfg(target_os = "macos")]
use std::ffi::OsString;

#[cfg(target_os = "macos")]
use crate::launch::{describe_status, split_cargo_args, strip_message_format};

/// Extract executable directories from Cargo's JSON build output.
///
/// Unparseable lines are ignored rather than fatal.
pub(crate) fn parse_executable_dirs(stdout: &str) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();

    for line in stdout.lines() {
        let Ok(message) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };

        let Some(executable) = message.get("executable").and_then(|v| v.as_str()) else {
            continue;
        };

        if let Some(dir) = Path::new(executable).parent()
            && !dirs.iter().any(|known| known == dir)
        {
            dirs.push(dir.to_path_buf());
        }
    }

    dirs
}

/// Ask Cargo where the selected executables were built.
///
/// The probe is a normal build, so the launch that follows is a cache hit.
#[cfg(target_os = "macos")]
pub(crate) fn executable_dirs(cef: &Path, cargo_args: &[OsString]) -> Result<Vec<PathBuf>> {
    let (build_args, _) = split_cargo_args(cargo_args);

    let output = crate::launch::cargo_command(cef, "build")?
        .args(strip_message_format(build_args))
        .arg("--message-format=json-render-diagnostics")
        .stderr(std::process::Stdio::inherit())
        .output()?;

    if !output.status.success() {
        let code = describe_status(&output.status);
        anyhow::bail!("cargo build failed (exit code: {code})");
    }

    Ok(parse_executable_dirs(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(dirs: &[PathBuf]) -> Vec<String> {
        dirs.iter().map(|d| d.display().to_string()).collect()
    }

    #[test]
    fn executable_directories_come_from_cargo_not_from_the_arguments() {
        let stream = r#"
{"reason":"compiler-artifact","executable":null,"target":{"name":"dep"}}
{"reason":"compiler-artifact","executable":"/w/target/aarch64-apple-darwin/release/app"}
{"reason":"build-finished","success":true}
"#;

        assert_eq!(
            names(&parse_executable_dirs(stream)),
            vec!["/w/target/aarch64-apple-darwin/release"],
            "the directory must be read from cargo, including the --target triple"
        );
    }

    #[test]
    fn every_distinct_output_directory_is_reported() {
        let stream = r#"
{"reason":"compiler-artifact","executable":"/w/target/debug/app"}
{"reason":"compiler-artifact","executable":"/w/target/debug/examples/demo"}
{"reason":"compiler-artifact","executable":"/w/target/debug/other"}
"#;

        assert_eq!(
            names(&parse_executable_dirs(stream)),
            vec!["/w/target/debug", "/w/target/debug/examples"],
            "binaries and examples land in different directories; both need GPU libraries"
        );
    }

    #[test]
    fn non_artifact_lines_are_ignored_rather_than_fatal() {
        let stream = "not json\n{\"reason\":\"build-finished\",\"success\":true}\n";

        assert!(parse_executable_dirs(stream).is_empty());
    }
}

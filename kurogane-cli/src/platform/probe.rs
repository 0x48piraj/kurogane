//! Locate Cargo-built executables for macOS GPU setup.
//!
//! Cargo reports the executable path directly, avoiding
//! assumptions about target and profile directories.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[cfg(target_os = "macos")]
use anyhow::Result;
#[cfg(target_os = "macos")]
use std::process::{Command, ExitStatus};

/// Splits a `cargo run` argument vector at the first bare `--`.
///
/// Everything to the left is Cargo's; everything to the right belongs to the
/// application. Only the left half can be replayed against `cargo build`.
pub(crate) fn split_cargo_args(args: &[OsString]) -> (&[OsString], &[OsString]) {
    match args.iter().position(|arg| arg == "--") {
        Some(index) => (&args[..index], &args[index + 1..]),
        None => (args, &[]),
    }
}

/// Strips any caller-supplied `--message-format` from probe arguments.
///
/// Cargo rejects multiple format flags and probes require JSON output to
/// parse artifact locations internally.
pub(crate) fn strip_message_format(args: &[OsString]) -> Vec<OsString> {
    let mut args_iter = args.iter();
    let mut kept = Vec::with_capacity(args.len());

    while let Some(arg) = args_iter.next() {
        let text = arg.to_string_lossy();

        if text == "--message-format" {
            let _ = args_iter.next();
        } else if !text.starts_with("--message-format=") {
            kept.push(arg.clone());
        }
    }

    kept
}

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
pub(crate) fn executable_dirs(cargo_args: &[OsString]) -> Result<Vec<PathBuf>> {
    let (build_args, _) = split_cargo_args(cargo_args);

    let output = Command::new("cargo")
        .arg("build")
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

/// Renders an exit status for a human-readable message.
#[cfg(target_os = "macos")]
fn describe_status(status: &ExitStatus) -> String {
    status
        .code()
        .map(|code| code.to_string())
        .unwrap_or_else(|| "signal".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    fn names_of(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    fn names(dirs: &[PathBuf]) -> Vec<String> {
        dirs.iter().map(|d| d.display().to_string()).collect()
    }

    #[test]
    fn arguments_without_a_separator_all_belong_to_cargo() {
        let all = args(&["--release", "--example", "foo"]);
        let (cargo, app) = split_cargo_args(&all);

        assert_eq!(names_of(cargo), vec!["--release", "--example", "foo"]);
        assert!(app.is_empty());
    }

    #[test]
    fn the_separator_hands_the_remainder_to_the_application() {
        let all = args(&["--release", "--", "--example", "foo"]);
        let (cargo, app) = split_cargo_args(&all);

        assert_eq!(names_of(cargo), vec!["--release"]);
        assert_eq!(
            names_of(app),
            vec!["--example", "foo"],
            "application flags must not be replayed against cargo build"
        );
    }

    #[test]
    fn only_the_first_separator_splits() {
        let all = args(&["--", "a", "--", "b"]);
        let (cargo, app) = split_cargo_args(&all);

        assert!(cargo.is_empty());
        assert_eq!(names_of(app), vec!["a", "--", "b"]);
    }

    #[test]
    fn a_leading_separator_leaves_cargo_nothing() {
        let all = args(&["--", "--release"]);
        let (cargo, _) = split_cargo_args(&all);

        assert!(
            cargo.is_empty(),
            "`--release` after `--` is the application's, not cargo's"
        );
    }

    #[test]
    fn message_format_is_stripped_in_both_spellings() {
        assert_eq!(
            names_of(&strip_message_format(&args(&[
                "--release",
                "--message-format",
                "human",
                "--example",
                "foo"
            ]))),
            vec!["--release", "--example", "foo"]
        );

        assert_eq!(
            names_of(&strip_message_format(&args(&[
                "--message-format=short",
                "--release"
            ]))),
            vec!["--release"]
        );
    }

    #[test]
    fn stripping_leaves_unrelated_arguments_untouched() {
        let original = args(&["--features", "a,b", "--target", "wasm32-unknown-unknown"]);

        assert_eq!(
            names_of(&strip_message_format(&original)),
            names_of(&original)
        );
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

//! Shared launch support for `dev` and `run`.
//!
//! Both commands prepare the Kurogane runtime and preserve the launched
//! application's exit status.
//!
//! This is a deliberate exception to the convention used elsewhere in the
//! CLI where a child process failure becomes an `anyhow` error and the
//! process exits `1`. Collapsing errors to `1` destroys valuable signals.
//!
//! Once control passes to the user's program, the program owns the exit code.

use anyhow::Result;
use cargo_metadata::{Package, Target, TargetKind};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

use kurogane_layout::{PackagingConfig, cef_install_dir, validate_cef_runtime};

use crate::tui;

/// Resolve the CEF runtime, installing it if necessary.
///
/// `CEF_PATH` overrides the managed install when valid for development convenience.
/// Provenance is deliberately not checked.
pub(crate) fn ensure_cef_runtime() -> Result<PathBuf> {
    let version = env!("KUROGANE_CEF_VERSION");

    let cef = std::env::var_os("CEF_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| cef_install_dir(version));

    tui::step("Checking Chromium engine");

    match validate_cef_runtime(&cef) {
        Ok(_) => {
            tui::success("Chromium engine ready");
            tui::field("path", tui::format_path(&cef));

            Ok(cef)
        }

        Err(err) => {
            tui::warn("Chromium runtime missing or invalid");
            tui::info("Initiating install process...");
            tui::field("reason", err);

            crate::install::run()?;

            // The installer populates the managed cache, not `cef`
            // Use the managed path after a failed validation
            let installed = cef_install_dir(version);

            validate_cef_runtime(&installed).map_err(|err| {
                anyhow::anyhow!(
                    "CEF runtime at {} is still invalid after install: {err}",
                    installed.display()
                )
            })?;

            tui::success("Chromium engine ready");
            tui::field("path", tui::format_path(&installed));

            Ok(installed)
        }
    }
}

/// Returns Kurogane's directory under Cargo's target directory.
///
/// Kurogane configures `cef-dll-sys` differently from plain cargo,
/// see [`crate::platform::cef_build_script_override`].
pub(crate) fn target_dir_in(base: &Path) -> PathBuf {
    base.join("kurogane")
}

/// Kurogane's target directory for the current project.
pub(crate) fn target_dir() -> Result<PathBuf> {
    let metadata = cargo_metadata::MetadataCommand::new().no_deps().exec()?;

    Ok(target_dir_in(metadata.target_directory.as_std_path()))
}

/// Constructs a Cargo command with Kurogane's CEF build configuration.
///
/// Uses a shared target directory and `CEF_PATH` across build, run and bundle.
pub(crate) fn cargo_command(cef: &Path, subcommand: &str) -> Result<Command> {
    let mut cmd = Command::new("cargo");
    cmd.arg(subcommand);

    // Skip cef-dll-sys's redundant runtime staging
    cmd.args(crate::platform::cef_build_script_override(cef)?);

    cmd.env("CEF_PATH", cef);

    cmd.env("CARGO_TARGET_DIR", target_dir()?);

    Ok(cmd)
}

/// Returns the package's first target of `kind`.
pub(crate) fn find_target(package: &Package, kind: TargetKind) -> Option<&Target> {
    package
        .targets
        .iter()
        .find(|target| target.kind.contains(&kind))
}

/// Runs the application with the Kurogane runtime environment, in the shape
/// its sandbox needs.
///
/// Reads `kurogane.toml` from the project root; only `sandbox = true` on
/// Windows changes the shape, see [`crate::sandbox`].
pub(crate) fn run_app(cef: &Path, cargo_args: &[OsString]) -> Result<ExitStatus> {
    let metadata = cargo_metadata::MetadataCommand::new().no_deps().exec()?;
    let project_root = metadata.workspace_root.as_std_path();
    let config = PackagingConfig::load(project_root)?;

    if !crate::sandbox::uses_bootstrap(&config.app) {
        return cargo_run(cef, cargo_args);
    }

    let package = metadata
        .root_package()
        .ok_or_else(|| anyhow::anyhow!("no root package in this workspace"))?;

    crate::sandbox::run(cef, cargo_args, package, project_root, &config.app)
}

/// Run Cargo with the Kurogane runtime environment.
fn cargo_run(cef: &Path, cargo_args: &[OsString]) -> Result<ExitStatus> {
    crate::platform::prepare_gpu_libraries(cef, cargo_args)?;

    let mut cmd = cargo_command(cef, "run")?;
    cmd.args(cargo_args);

    // Configure platform-specific runtime loading for the launched process
    crate::platform::configure_runtime_env(&mut cmd, cef)?;

    tui::blank();
    tui::step("Launching application");
    tui::blank();

    Ok(cmd.status()?)
}

/// Terminates the process, preserving the child's exit status.
pub(crate) fn exit_with(status: ExitStatus) -> ! {
    use std::io::Write;

    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();

    std::process::exit(exit_code(status))
}

/// The exit code a shell reports for `status`: a program's own code, or 128
/// plus the number of the signal that killed it.
fn exit_code(status: ExitStatus) -> i32 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;

        if let Some(signal) = status.signal() {
            return 128 + signal;
        }
    }

    // Windows always reports a code
    status.code().unwrap_or(1)
}

/// Formats an argument vector for display.
pub(crate) fn describe_args(args: &[OsString]) -> String {
    args.iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Renders an exit status for a human-readable message.
pub(crate) fn describe_status(status: &ExitStatus) -> String {
    status
        .code()
        .map(|code| code.to_string())
        .unwrap_or_else(|| "signal".into())
}

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

/// Strips any caller-supplied `--message-format` from build arguments.
///
/// Cargo rejects multiple format flags, and a build whose artifacts are read
/// back needs JSON output of its own.
pub(crate) fn strip_message_format(args: &[OsString]) -> Vec<OsString> {
    let mut args_iter = args.iter();
    let mut kept = Vec::with_capacity(args.len());

    while let Some(arg) = args_iter.next() {
        let text = arg.to_string_lossy();

        if text == "--message-format" {
            // Its value goes too
            args_iter.next();
        } else if !text.starts_with("--message-format=") {
            kept.push(arg.clone());
        }
    }

    kept
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
}

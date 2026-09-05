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
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::{Command, ExitStatus};

use kurogane_layout::{cef_install_dir, validate_cef_runtime};

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

/// Run Cargo with the Kurogane runtime environment.
pub(crate) fn cargo_run(cef: &std::path::Path, cargo_args: &[OsString]) -> Result<ExitStatus> {
    crate::platform::prepare_gpu_libraries(cef, cargo_args)?;

    let mut cmd = Command::new("cargo");
    cmd.arg("run");
    cmd.args(cargo_args);
    cmd.env("CEF_PATH", cef);

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

    // Default (SIGINT / Ctrl-C convention) if killed by a signal without a code
    let code = status.code().unwrap_or(130);

    std::process::exit(code)
}

/// Formats an argument vector for display.
pub(crate) fn describe_args(args: &[OsString]) -> String {
    args.iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

//! Development-mode application launcher.
//!
//! This module ensures a valid CEF runtime is available and launches the
//! application with the environment required for local runtime discovery
//! and dynamic linking.

use anyhow::Result;
use std::ffi::OsString;
use std::process::Command;
use kurogane_layout::{cef_install_dir, validate_cef_runtime};

use crate::tui;

pub fn run(cargo_args: Vec<OsString>) -> Result<()> {
    tui::section("Kurogane Dev");

    let version = env!("KUROGANE_CEF_VERSION");

    // CEF_PATH overrides the managed install for development convenience
    // Provenance is not checked; dev mode only needs a valid runtime
    let cef = std::env::var_os("CEF_PATH")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| cef_install_dir(version));

    tui::step("Checking Chromium engine");

    match validate_cef_runtime(&cef) {
        Ok(_) => {
            tui::success("Chromium engine ready");
            tui::field("path", tui::format_path(&cef));
        }

        Err(err) => {
            tui::warn("Chromium runtime missing or invalid");
            tui::info("Initiating install process...");
            tui::field("reason", err);

            crate::install::run()?;
        }
    }

    // Pass env to build step
    let mut cmd = Command::new("cargo");
    cmd.arg("run");

    for arg in cargo_args {
        cmd.arg(arg);
    }

    cmd.env("CEF_PATH", &cef);

    // OS-specific runtime linking

    #[cfg(target_os = "linux")]
    {
        let ld = std::env::var("LD_LIBRARY_PATH").unwrap_or_default();
        let ld = if ld.is_empty() {
            cef.display().to_string()
        } else {
            format!("{}:{}", cef.display(), ld)
        };
        cmd.env("LD_LIBRARY_PATH", ld);
    }

    #[cfg(target_os = "windows")]
    {
        let path = std::env::var("PATH").unwrap_or_default();
        let path = if path.is_empty() {
            cef.display().to_string()
        } else {
            format!("{};{}", cef.display(), path)
        };
        cmd.env("PATH", path);
    }

    #[cfg(target_os = "macos")]
    {
        let mut dyld = std::env::var("DYLD_FALLBACK_LIBRARY_PATH").unwrap_or_default();
        dyld = format!("{}:{}", cef.display(), dyld);
        cmd.env("DYLD_FALLBACK_LIBRARY_PATH", dyld);
    }

    tui::blank();
    tui::step("Launching application");

    let status = cmd.status()?;

    if !status.success() {
        let code = status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "signal".into());
        anyhow::bail!("Application failed (exit code: {code})");
    }

    tui::blank();
    tui::success("Application exited");

    Ok(())
}

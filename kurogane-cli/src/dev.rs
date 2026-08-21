use anyhow::Result;
use std::ffi::OsString;
use std::process::Command;
use kurogane_layout::{cef_install_dir, validate_cef_runtime};

use crate::tui;

pub fn run(cargo_args: Vec<OsString>) -> Result<()> {
    tui::section("Kurogane Dev");

    let version = env!("KUROGANE_CEF_VERSION");

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

    //
    // OS-specific runtime linking
    //
    #[cfg(target_os = "linux")]
    {
        let mut ld = std::env::var("LD_LIBRARY_PATH").unwrap_or_default();
        ld = format!("{}:{}", cef.display(), ld);
        cmd.env("LD_LIBRARY_PATH", ld);
    }

    #[cfg(target_os = "windows")]
    {
        let mut path = std::env::var("PATH").unwrap_or_default();
        path = format!("{};{}", cef.display(), path);
        cmd.env("PATH", path);
    }

    #[cfg(target_os = "macos")]
    {
        let mut dyld = std::env::var("DYLD_FALLBACK_LIBRARY_PATH").unwrap_or_default();
        dyld = format!("{}:{}", cef.display(), dyld);
        cmd.env("DYLD_FALLBACK_LIBRARY_PATH", dyld);
    }

    println!();
    tui::step("Launching application");

    let status = cmd.status()?;

    if !status.success() {
        anyhow::bail!("Application failed");
    }

    println!();
    tui::success("Application exited");

    Ok(())
}

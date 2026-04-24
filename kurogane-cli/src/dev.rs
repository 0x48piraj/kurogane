use anyhow::Result;
use std::ffi::OsString;
use std::process::Command;

use crate::tui;

pub fn run(cargo_args: Vec<OsString>) -> Result<()> {
    tui::section("Kurogane Dev");

    let version = env!("KUROGANE_CEF_VERSION");
    let cef = dirs::home_dir()
        .expect("no home dir")
        .join(".local/share/cef")
        .join(version);

    tui::step("Checking Chromium engine");

    if !cef.exists() {
        tui::warn("Chromium engine not found");
        tui::info("Initiating install process");
        crate::install::run()?;
    } else {
        tui::success("Chromium engine ready");
        tui::field("path", tui::format_path(&cef));
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

use anyhow::{Result, bail};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use crate::tui;

pub fn run() -> Result<()> {
    tui::section("Kurogane Bundle");

    // Ensure release build
    tui::step("Building release...");

    let status = Command::new("cargo")
        .arg("build")
        .arg("--release")
        .status()?;

    if !status.success() {
        bail!("Release build failed");
    }

    let target = PathBuf::from("target/release");

    // Find executable
    tui::step("Locating executable...");
    let exe = find_exe(&target)?;
    tui::field("binary", tui::format_path(&exe));

    // Prepare destination
    let dist = PathBuf::from("dist");

    if dist.exists() {
        tui::step("Cleaning build directory...");
        fs::remove_dir_all(&dist)?;
    }

    fs::create_dir_all(&dist)?;

    // Copy executable
    tui::step("Copying executable...");

    let exe_name = exe.file_name().unwrap();
    fs::copy(&exe, dist.join(exe_name))?;

    // Copy frontend
    let content = PathBuf::from("content");
    if content.exists() {
        tui::step("Copying frontend...");
        copy_dir(&content, &dist.join("content"))?;
    } else {
        tui::warn("No content/ directory found");
    }

    // Copy CEF
    tui::step("Copying Chromium engine...");

    let cef_src = find_cef()?;
    let cef_dst = dist.join("cef");

    tui::field("source", tui::format_path(&cef_src));

    copy_dir(&cef_src, &cef_dst)?;

    println!();
    tui::success("Bundle ready");
    tui::field("path", "./dist");

    Ok(())
}

fn find_exe(dir: &PathBuf) -> Result<PathBuf> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();

        if cfg!(target_os = "windows") {
            if path.extension().map(|e| e == "exe").unwrap_or(false) {
                return Ok(path);
            }
        } else if path.is_file() && path.extension().is_none() {
            return Ok(path);
        }
    }

    bail!("No executable found in {:?}", dir);
}

fn copy_dir(src: &PathBuf, dst: &PathBuf) -> Result<()> {
    fs::create_dir_all(dst)?;

    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let dest = dst.join(entry.file_name());

        if path.is_dir() {
            copy_dir(&path, &dest)?;
        } else {
            fs::copy(&path, &dest)?;
        }
    }

    Ok(())
}

fn find_cef() -> Result<PathBuf> {
    // Next to exe
    let local = PathBuf::from("cef");
    if local.exists() {
        return Ok(local);
    }

    // User install
    if let Some(home) = dirs::home_dir() {
        let path = home.join(".local/share/cef");
        if path.exists() {
            return Ok(path);
        }
    }

    anyhow::bail!("Chromium engine not found. Run 'kurogane install'.");
}

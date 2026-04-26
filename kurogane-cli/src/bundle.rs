use anyhow::{Result, bail};
use std::fs;
use std::path::{PathBuf, Path};
use std::process::Command;
use cargo_metadata::{MetadataCommand, TargetKind};

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

    // Find executable
    tui::step("Locating executable...");
    let exe = find_exe()?;
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

    tui::field("source", tui::format_path(&cef_src));
    tui::step("Preparing runtime...");

    copy_cef_bundle(&cef_src, &dist)?;

    tui::step("Verifying bundle");

    let bin = dist.join(exe_name);
    let index = dist.join("content/index.html");

    if !bin.exists() {
        anyhow::bail!("Bundle failed: binary missing");
    }

    if !index.exists() {
        anyhow::bail!("Bundle failed: content/index.html missing");
    }

    #[cfg(target_os = "windows")]
    {
        let libcef = dist.join("libcef.dll");

        if !libcef.exists() {
            tui::error("Bundle failed: CEF not copied correctly");
            anyhow::bail!("libcef.dll missing");
        }
    }

    #[cfg(target_os = "linux")]
    {
        let cef = dist.join("cef");

        if !cef.exists() {
            tui::error("Bundle failed: CEF not copied correctly");
            anyhow::bail!("cef/ directory missing");
        }
    }

    tui::success("Bundle verified");
    tui::field("binary", tui::format_path(&bin));
    tui::field("entry", tui::format_path(&index));

    println!();
    tui::success("Bundle ready");
    tui::field("path", "./dist");

    Ok(())
}

fn find_exe() -> Result<PathBuf> {
    let metadata = MetadataCommand::new().exec()?;

    let pkg = metadata.root_package()
        .ok_or_else(|| anyhow::anyhow!("No root package"))?;

    let target_dir = metadata.target_directory.join("release");

    // Find binary target
    let target = pkg.targets.iter()
        .find(|t| t.kind.contains(&TargetKind::Bin))
        .ok_or_else(|| anyhow::anyhow!("No binary target found"))?;

    let exe_name = &target.name;

    let exe_path = if cfg!(target_os = "windows") {
        target_dir.join(format!("{exe_name}.exe"))
    } else {
        target_dir.join(exe_name)
    };

    if exe_path.exists() {
        Ok(exe_path.into_std_path_buf()) 
    } else {
        bail!("Executable not found: {:?}", exe_path)
    }
}

fn copy_dir(src: &std::path::Path, dst: &Path) -> Result<()> {
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
    let version = env!("KUROGANE_CEF_VERSION");

    if let Some(home) = dirs::home_dir() {
        let path = home.join(".local/share/cef").join(version);
        if path.exists() {
            return Ok(path);
        }
    }

    anyhow::bail!("Chromium engine not found. Run 'kurogane install'.");
}

/// Copy Chromium Embedded Framework for Windows.
///
/// On Windows, the dynamic loader automatically searches for DLLs
/// in the same directory as the executable.
///
/// Because of this, we flatten the CEF directory and copy all
/// required resource files directly into dist/.
///
/// This avoids any runtime configuration (no PATH hacks, no env vars)
/// and ensures the application is self-contained.
#[cfg(target_os = "windows")]
fn copy_cef_bundle(src: &PathBuf, dist: &PathBuf) -> Result<()> {
    copy_dir(src, dist)?; // flatten
    Ok(())
}

/// Copy Chromium Embedded Framework for Linux.
///
/// On Linux, the dynamic linker does not search the executable
/// directory for shared libraries by default.
///
/// Instead, it relies on:
///   - RPATH / RUNPATH embedded in the binary
///   - LD_LIBRARY_PATH environment variable
///   - System library paths
///
/// To keep the bundle clean and predictable, we:
///   - Place CEF inside dist/cef/
///   - Use RPATH ($ORIGIN/cef) so the binary can locate it at runtime
///
/// This avoids requiring environment variables or wrapper scripts.
///
/// Additionally, CEF requires chrome-sandbox to have setuid permissions
/// for proper sandboxing. Without this, CEF may fail silently.
#[cfg(target_os = "linux")]
fn copy_cef_bundle(src: &PathBuf, dist: &PathBuf) -> Result<()> {
    let cef_dst = dist.join("cef");
    copy_dir(src, &cef_dst)?;

    // Sandbox permissions (required by CEF)
    let sandbox = cef_dst.join("chrome-sandbox");
    let _ = Command::new("chmod")
        .arg("4755")
        .arg(&sandbox)
        .status();

    Ok(())
}

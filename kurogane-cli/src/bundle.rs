use anyhow::{Result, bail};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use cargo_metadata::{MetadataCommand, TargetKind};
use kurogane_layout::{BundleLayout, validate_cef_root};

use crate::tui;

pub fn run(debug: bool) -> Result<()> {
    tui::section("Kurogane Bundle");

    // Ensure release build
    tui::step("Building release...");

    let mut cmd = Command::new("cargo");

    cmd.arg("build");

    if debug {
        cmd.arg("--features").arg("kurogane/debug");
    } else {
        cmd.arg("--release");
    }

    let status = cmd.status()?;

    if !status.success() {
        bail!("Release build failed");
    }

    // Find executable
    tui::step("Locating executable...");
    let exe = find_exe(debug)?;
    tui::field("binary", tui::format_path(&exe));

    // Prepare destination
    let dist = PathBuf::from("dist");

    let layout = BundleLayout::new(&dist);

    tui::step("Preparing bundle...");
    layout.prepare()?;

    tui::step("Copying executable...");

    let exe_name = exe.file_name().unwrap();

    let runtime_bin = layout.executable_path(exe_name);

    // Copy executable
    fs::copy(&exe, &runtime_bin)?;

    #[cfg(target_os = "linux")]
    layout.write_launcher(exe_name)?;

    tui::field("runtime", tui::format_path(&runtime_bin));

    let content = PathBuf::from("content");

    if content.exists() {
        tui::step("Copying frontend...");

        // Copy frontend
        layout.install_frontend(&content)?;
    } else {
        tui::warn("No content/ directory found");
    }

    tui::step("Copying Chromium engine...");

    let detected = kurogane_layout::detect_cef_root()
        .map_err(|e| anyhow::anyhow!(e))?;

    validate_cef_root(&detected.root)?;

    // Copy CEF
    layout.install_cef(&detected.root)?;

    tui::step("Verifying bundle");

    layout.verify(exe_name)?;

    tui::success("Bundle verified");

    tui::field(
        "binary",
        tui::format_path(
            &layout.launcher_path(exe_name)
        ),
    );

    tui::field(
        "entry",
        tui::format_path(
            &layout.content_dir().join("index.html")
        ),
    );

    println!();
    tui::success("Bundle ready");
    tui::field("path", "./dist");

    Ok(())
}

fn find_exe(debug: bool) -> Result<PathBuf> {
    let metadata = MetadataCommand::new().exec()?;

    let pkg = metadata.root_package()
        .ok_or_else(|| anyhow::anyhow!("No root package"))?;

    let profile = if debug { "debug" } else { "release" };
    let target_dir = metadata.target_directory.join(profile);

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

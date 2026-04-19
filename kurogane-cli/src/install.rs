use anyhow::Result;
use download_cef::{CefIndex, DEFAULT_TARGET};
use std::time::Duration;
use std::path::{Path, PathBuf};

use crate::tui;

pub fn run() -> Result<()> {
    tui::section("Kurogane installer");

    let install_dir = default_install_dir(); // ~/.local/share/cef

    if install_dir.exists() {
        tui::success("Chromium engine already installed");
        tui::field("path", tui::format_path(&install_dir));
        return Ok(());
    }

    let cef_version = env!("KUROGANE_CEF_VERSION").to_string();

    tui::step("Resolving version...");
    tui::field("chromium", &cef_version);

    let parent = install_dir.parent().unwrap(); // ~/.local/share
    std::fs::create_dir_all(parent)?;

    let index = CefIndex::download()?;
    let platform = index.platform(DEFAULT_TARGET)?;
    let version = platform.version(&cef_version)?;

    tui::step("Downloading Chromium engine...");

    let archive = version.download_archive_with_retry(
        parent,
        true,
        Duration::from_secs(15),
        3,
    )?;

    tui::step("Extracting...");

    let extracted = download_cef::extract_target_archive(
        DEFAULT_TARGET,
        &archive,
        parent,
        true,
    )?;

    // Write archive.json
    version.minimal()?.write_archive_json(&extracted)?;

    // Replace existing install (safety, though we already early-returned)
    if install_dir.exists() {
        tui::step("Removing old install...");
        std::fs::remove_dir_all(&install_dir)?;
    }

    tui::step("Installing...");
    tui::field("path", tui::format_path(&install_dir));

    std::fs::rename(&extracted, &install_dir)?;

    let _ = std::fs::remove_file(&archive);

    println!();

    tui::success("Chromium engine installed");
    tui::field("path", tui::format_path(&install_dir));

    print_env_instructions(&install_dir);

    Ok(())
}

fn default_install_dir() -> PathBuf {
    dirs::home_dir()
        .expect("no home dir")
        .join(".local/share/cef")
}

fn print_env_instructions(root: &Path) {
    println!();
    tui::section("Environment setup (optional)");

    #[cfg(target_os = "windows")]
    {
        tui::info("PowerShell");
        println!(r#"    $env:CEF_PATH="{}""#, tui::format_path(root));
        println!(r#"    $env:PATH="$env:PATH;$env:CEF_PATH""#);
    }

    #[cfg(target_os = "linux")]
    {
        println!(r#"    export CEF_PATH="{}""#, tui::format_path(root));
        println!(r#"    export LD_LIBRARY_PATH="$LD_LIBRARY_PATH:$CEF_PATH""#);
        println!();
        tui::warn("Run once");
        println!(
            "    sudo chown root:root {}/chrome-sandbox",
            tui::format_path(root)
        );
        println!(
            "    sudo chmod 4755 {}/chrome-sandbox",
            tui::format_path(root)
        );
    }

    #[cfg(target_os = "macos")]
    {
        println!(r#"    export CEF_PATH="{}""#, tui::format_path(root));
        println!(
            r#"    export DYLD_FALLBACK_LIBRARY_PATH="$DYLD_FALLBACK_LIBRARY_PATH:$CEF_PATH:$CEF_PATH/Chromium Embedded Framework.framework/Libraries""#
        );
    }

    println!();
    tui::step("Restart your terminal after running these commands");
    tui::step("Then run: kurogane dev");
    println!();
}

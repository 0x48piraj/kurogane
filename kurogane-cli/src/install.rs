use anyhow::Result;
use download_cef::{CefIndex, DEFAULT_TARGET};
use std::time::Duration;
use std::path::PathBuf;

use crate::tui;

pub fn run() -> Result<()> {
    tui::section("Kurogane installer");

    let cef_version = env!("KUROGANE_CEF_VERSION").to_string();
    let install_dir = install_dir_for(&cef_version);

    if install_dir.exists() {
        tui::success("Chromium engine already installed");
        tui::field("version", &cef_version);
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

    tui::step("Installing...");
    tui::field("path", tui::format_path(&install_dir));

    std::fs::rename(&extracted, &install_dir)?;

    let _ = std::fs::remove_file(&archive);

    println!();

    tui::success("Chromium engine installed");
    tui::field("path", tui::format_path(&install_dir));

    Ok(())
}

fn install_dir_for(version: &str) -> PathBuf {
    dirs::home_dir()
        .expect("no home dir")
        .join(".local/share/cef")
        .join(version)
}

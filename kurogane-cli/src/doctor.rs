use anyhow::Result;
use std::path::PathBuf;

use crate::tui;

pub fn run() -> Result<()> {
    tui::section("Kurogane Doctor");

    let mut warn = 0;
    let mut fail = 0;

    // Check CEF installation
    let cef_path = dirs::home_dir()
        .map(|h| h.join(".local/share/cef"))
        .unwrap_or(PathBuf::from("~/.local/share/cef"));

    if cef_path.exists() {
        tui::success("CEF installation");
        tui::field("path", tui::format_path(&cef_path));
    } else {
        tui::error("CEF not found");
        tui::field("expected", tui::format_path(&cef_path));
        tui::info("Run: kurogane install");
        fail += 1;
    }

    println!();

    // Check CEF_PATH env
    match std::env::var("CEF_PATH") {
        Ok(v) => {
            tui::success("Environment");
            tui::field("CEF_PATH", v);
        }
        Err(_) => {
            tui::warn("Environment");
            tui::field("CEF_PATH", "not set");
            tui::step("Resolved to default install path");
            warn += 1;
        }
    }

    println!();

    // Check Cargo.toml
    if std::path::Path::new("Cargo.toml").exists() {
        tui::success("Cargo project detected");
    } else {
        tui::error("Not inside a Rust project");
        fail += 1;
    }

    // Check project structure
    if std::path::Path::new("content").exists() {
        tui::success("Using default directory");
    } else {
        tui::warn("Default content directory not found");
        tui::field("default", "./content");
        warn += 1;
    }

    tui::section("Summary");

    if fail > 0 {
        tui::error("System status: Non-operational");
    } else if warn > 0 {
        tui::warn("System status: Degraded (warnings detected)");
    } else {
        tui::success("System status: Operational");
    }

    println!();

    Ok(())
}

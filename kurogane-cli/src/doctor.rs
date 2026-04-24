use anyhow::Result;
use std::path::PathBuf;

use crate::tui;

struct ToolCheck {
    name: &'static str,
    cmd: &'static str,
    hint: &'static str,
}

fn required_tools() -> Vec<ToolCheck> {
    if cfg!(windows) {
        vec![
            ToolCheck {
                name: "MSVC",
                cmd: "cl",
                hint: "not available",
            },
            ToolCheck {
                name: "CMake",
                cmd: "cmake",
                hint: "not found",
            },
            ToolCheck {
                name: "Ninja",
                cmd: "ninja",
                hint: "not found",
            },
        ]
    } else {
        vec![
            ToolCheck {
                name: "C compiler (cc)",
                cmd: "cc",
                hint: "No C compiler found. Run: 'sudo apt install build-essential' # or distro equivalent",
            },
            ToolCheck {
                name: "CMake",
                cmd: "cmake",
                hint: "Ninja not found. Run: 'sudo apt install cmake' # or distro equivalent",
            },
        ]
    }
}

fn probe(cmd: &str) -> bool {
    std::process::Command::new(cmd)
        .arg("--version")
        .output()
        .is_ok()
}

pub fn run() -> Result<()> {
    tui::section("Kurogane Doctor");

    let mut warn = 0;
    let mut fail = 0;

    // Check CEF installation
    let version = env!("KUROGANE_CEF_VERSION");

    let cef_path = dirs::home_dir()
        .map(|h| h.join(".local/share/cef").join(version))
        .unwrap_or_else(|| PathBuf::from("~/.local/share/cef").join(version));

    if cef_path.exists() {
        tui::success("CEF installation");
        tui::field("version", version);
        tui::field("path", tui::format_path(&cef_path));
    } else {
        tui::error("CEF not found");
        tui::field("required", version);
        tui::field("expected", tui::format_path(&cef_path));
        tui::info("Run: kurogane install");
        fail += 1;
    }

    let root = dirs::home_dir()
        .map(|h| h.join(".local/share/cef"));

    if let Some(root) = root {
        if let Ok(entries) = std::fs::read_dir(&root) {
            let versions: Vec<_> = entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect();

            if !versions.is_empty() {
                println!();
                tui::info("Installed versions");
                for v in versions {
                    tui::field("cef", v);
                }
            }
        }
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
            tui::step("Using versioned runtime path");
        }
    }

    tui::section("Toolchain");

    let tools = required_tools();

    let mut missing = Vec::new();

    for tool in tools {
        if !probe(tool.cmd) {
            missing.push(tool);
            fail += 1;
        } else {
            tui::success(tool.name);
        }
    }

    if !missing.is_empty() {
        // Grouped hints
        if cfg!(windows) {
            if std::env::var("VCINSTALLDIR").is_ok() {
                tui::error("Missing Visual Studio components");
                tui::field("hint", "Install C++ workload via Visual Studio Installer");
            } else {
                tui::error("Build toolchain not available");
                tui::field("hint", "Run from 'Developer Command Prompt for VS' (search in Start menu)");
            }
        } else {
            tui::error("Build toolchain not found");
        }

        println!();

        tui::info("Components:");

        // Structured details
        for tool in &missing {
            tui::field(tool.name, tool.hint);
        }
    }

    tui::section("Project");

    // Check Cargo.toml
    if std::path::Path::new("Cargo.toml").exists() {
        tui::success("Cargo project detected");
    } else {
        tui::error("Not inside a Rust project");
        fail += 1;
    }

    // Check project structure
    if std::path::Path::new("content").exists() {
        tui::success("Using default frontend directory");
    } else {
        tui::warn("Default content directory not found");
        tui::field("default", "./content");
        warn += 1;
    }

    tui::section("Summary");

    match (fail, warn) {
        (f, _) if f > 0 => tui::error("System status: Non-operational"),
        (_, w) if w > 0 => tui::warn("System status: Degraded (warnings detected)"),
        _ => tui::success("System status: Operational"),
    }

    println!();

    Ok(())
}

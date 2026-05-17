use anyhow::Result;
use kurogane_layout::{detect_cef_root, install_root, installed_cef_root, validate_cef_root};

use crate::tui;
use crate::collector;

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
                hint: "Install Visual Studio C++ build tools",
            },
            ToolCheck {
                name: "CMake",
                cmd: "cmake",
                hint: "Install CMake",
            },
            ToolCheck {
                name: "Ninja",
                cmd: "ninja",
                hint: "Install Ninja build system",
            },
        ]
    } else {
        vec![
            ToolCheck {
                name: "C compiler (cc)",
                cmd: "cc",
                hint: "Install build-essential or your distro's compiler toolchain",
            },
            ToolCheck {
                name: "CMake",
                cmd: "cmake",
                hint: "Install CMake",
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

pub fn run(json: bool) -> Result<()> {

    // JSON mode
    if json {
        let report = collector::collect_all();
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    tui::section("Kurogane Doctor");

    let mut warn = 0;
    let mut fail = 0;

    // Check CEF installation
    let version = env!("KUROGANE_CEF_VERSION");

    // Managed installed runtime
    match installed_cef_root(version) {
        Some(root) => {
            match validate_cef_root(&root) {
                Ok(_) => {
                    tui::success("Managed Chromium runtime");
                    tui::field("version", version);
                    tui::field("path", tui::format_path(&root));
                }

                Err(e) => {
                    tui::error("Managed Chromium runtime invalid");
                    tui::field("reason", e);

                    fail += 1;
                }
            }
        }

        None => {
            tui::error("Managed Chromium runtime not found");

            tui::field("required", version);

            tui::field("expected", tui::format_path(&install_root().join(version)));

            tui::info("Run: kurogane install");

            warn += 1;
        }
    }

    let root = install_root();

    if let Ok(entries) = std::fs::read_dir(&root) {
        let versions: Vec<_> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();

        if !versions.is_empty() {
            println!();

            tui::info("Installed versions");

            for version in versions {
                tui::field("cef", version);
            }
        }
    }

    println!();

    tui::section("Runtime Resolution");

    match detect_cef_root() {
        Ok(detected) => {
            match validate_cef_root(&detected.root) {
                Ok(_) => {
                    tui::success("Active runtime resolved");

                    tui::field("path", tui::format_path(&detected.root));

                    tui::field("mode", format!("{:?}", detected.mode));
                }

                Err(e) => {
                    tui::error("Resolved runtime invalid");

                    tui::field("reason", e);

                    fail += 1;
                }
            }
        }

        Err(_) => {
            tui::warn("No usable Chromium runtime found");

            tui::info(
                "Applications may fail to launch outside managed environments"
            );

            warn += 1;
        }
    }

    println!();

    // Check CEF_PATH env
    match std::env::var("CEF_PATH") {
        Ok(v) => {
            tui::success("Environment override");
            tui::field("CEF_PATH", v);
        }

        Err(_) => {
            tui::warn("Environment override");
            tui::field("CEF_PATH", "not set");
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
                tui::error("Visual Studio environment unavailable");
                tui::field("hint", "Run inside Developer Command Prompt for Visual Studio");
            }
        } else {
            tui::error("Build toolchain not found");
        }

        println!();

        tui::info("Missing components");

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

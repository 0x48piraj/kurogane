//! Environment and installation diagnostics.
//!
//! This module validates the local CEF installation, runtime discovery,
//! build toolchain and project structure and presents the resulting health
//! report to the user.

use anyhow::Result;
use cargo_metadata::MetadataCommand;
use kurogane_layout::{
    detect_cef_root_with_version, install_root, installed_cef_root, read_provenance,
    validate_cef_runtime,
};

use crate::collector;
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
    } else if cfg!(target_os = "macos") {
        vec![
            ToolCheck {
                name: "Xcode Command Line Tools (clang)",
                cmd: "clang",
                hint: "Install Command Line Tools: xcode-select --install",
            },
            ToolCheck {
                name: "CMake",
                cmd: "cmake",
                hint: "Install CMake",
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
        Some(root) => match validate_cef_runtime(&root) {
            Ok(_) => {
                tui::success("Managed Chromium runtime");
                tui::field("version", version);
                tui::field("path", tui::format_path(&root));

                if let Ok(Some(p)) = read_provenance(&root) {
                    tui::field("artifact", p.artifact);
                }
            }

            Err(e) => {
                tui::error("Managed Chromium runtime invalid");
                tui::field("reason", e);

                fail += 1;
            }
        },

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
            .flatten()
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();

        if !versions.is_empty() {
            tui::blank();

            tui::info("Installed versions");

            for version in versions {
                tui::field("cef", version);
            }
        }
    }

    tui::blank();

    tui::section("Runtime Resolution");

    match detect_cef_root_with_version(Some(version)) {
        Ok(detected) => match validate_cef_runtime(&detected.root) {
            Ok(_) => {
                tui::success("Active runtime resolved");

                tui::field("path", tui::format_path(&detected.root));

                tui::field("mode", detected.mode.to_string());

                if let Some(p) = &detected.provenance {
                    tui::field("provenance", p.artifact.clone());
                } else {
                    tui::warn("Provenance unknown (no archive.json)");
                }
            }

            Err(e) => {
                tui::error("Resolved runtime invalid");

                tui::field("reason", e);

                fail += 1;
            }
        },

        Err(_) => {
            tui::warn("No usable Chromium runtime found");

            tui::info("Applications may fail to launch outside managed environments");

            warn += 1;
        }
    }

    tui::blank();

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
                tui::field(
                    "hint",
                    "Run inside Developer Command Prompt for Visual Studio",
                );
            }
        } else {
            tui::error("Build toolchain not found");
        }

        tui::blank();

        tui::info("Missing components");

        // Structured details
        for tool in &missing {
            tui::field(tool.name, tool.hint);
        }
    }

    tui::section("Project");

    // Resolve workspace root
    let workspace_root = MetadataCommand::new().exec().ok().map(|m| {
        let root = m.workspace_root.into_std_path_buf();
        tui::success("Cargo workspace detected");
        tui::field("root", tui::format_path(&root));
        root
    });

    if workspace_root.is_none() {
        tui::error("Not inside a Cargo workspace");
        fail += 1;
    }

    // Check configured frontend from the resolved workspace root
    let packaging_config = workspace_root.as_deref().and_then(|root| {
        kurogane_layout::PackagingConfig::load(root)
            .ok()
            .map(|c| (root, c))
    });

    if let Some((root, config)) = packaging_config {
        match config
            .app
            .frontend
            .as_deref()
            .map(|path| kurogane_layout::anchor_path(root, path))
        {
            Some(frontend) if frontend.exists() => {
                tui::success("Frontend directory");
                tui::field("path", tui::format_path(&frontend));
            }
            Some(frontend) => {
                tui::warn("Configured frontend directory not found");
                tui::field("path", tui::format_path(&frontend));
                warn += 1;
            }
            None => tui::info("No frontend directory configured in kurogane.toml"),
        }
    } else {
        tui::info("No kurogane.toml found");
    }

    tui::section("Summary");

    match (fail, warn) {
        (f, _) if f > 0 => tui::error("System status: Non-operational"),
        (_, w) if w > 0 => tui::warn("System status: Degraded (warnings detected)"),
        _ => tui::success("System status: Operational"),
    }

    tui::blank();

    Ok(())
}

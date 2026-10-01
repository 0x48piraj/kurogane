//! Environment and installation diagnostics.
//!
//! This module validates the local CEF installation, runtime discovery,
//! build toolchain and project structure and presents the resulting health
//! report to the user.

use anyhow::Result;
use cargo_metadata::MetadataCommand;
use kurogane_layout::{
    CefSource, install_root, installed_cef_root, read_provenance, resolve_cef_for_bundle,
    validate_cef_runtime,
};

use crate::collector;
use crate::tui;

struct ToolCheck {
    name: &'static str,
    cmd: &'static str,
    hint: &'static str,
}

/// The tools a CEF build needs on the running host.
///
/// `cef-dll-sys` compiles `libcef_dll_wrapper` through CMake's Ninja generator
/// on Windows and macOS. Its Linux branch only stages the runtime and emits
/// link directives, so neither tool is involved there.
///
/// macOS needs both for Kurogane's own shared wrapper build as well,
/// see [`crate::platform`].
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
            ToolCheck {
                name: "Ninja",
                cmd: "ninja",
                hint: "Install Ninja build system",
            },
        ]
    } else {
        vec![ToolCheck {
            name: "C compiler (cc)",
            cmd: "cc",
            hint: "Install build-essential or your distro's compiler toolchain",
        }]
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

            fail += 1;
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

    // What `kurogane dev`, `run` and `build` start the application with
    let (dev, source) = crate::launch::dev_cef_root();
    match validate_cef_runtime(&dev) {
        Ok(_) => {
            tui::success("dev, run and build use");
            tui::field("path", tui::format_path(&dev));
            tui::field("source", source);
        }

        Err(e) => {
            tui::warn("dev, run and build find no usable runtime; they install one");
            tui::field("path", tui::format_path(&dev));
            tui::field("source", source);
            tui::field("reason", e);

            warn += 1;
        }
    }

    tui::blank();

    // What `kurogane bundle` packages, with verified provenance
    match resolve_cef_for_bundle(version) {
        Ok(resolved) => {
            tui::success("bundle packages");
            tui::field("path", tui::format_path(&resolved.root));
            tui::field(
                "source",
                match resolved.source {
                    CefSource::EnvironmentOverride => "CEF_PATH",
                    CefSource::ManagedCache => "managed install",
                },
            );

            if let Some(p) = &resolved.provenance {
                tui::field("provenance", p.artifact.clone());
            }
        }

        Err(e) => {
            tui::warn("bundle has no runtime it can package");
            tui::field("reason", e);

            warn += 1;
        }
    }

    tui::blank();

    tui::info("A bundled application uses only the runtime inside its bundle");

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
    let workspace_root = match MetadataCommand::new().no_deps().exec() {
        Ok(metadata) => {
            let root = metadata.workspace_root.into_std_path_buf();
            tui::success("Cargo workspace detected");
            tui::field("root", tui::format_path(&root));
            Some(root)
        }
        Err(e) => {
            tui::error("Not inside a Cargo workspace");
            tui::field("cause", e);
            fail += 1;
            None
        }
    };

    // Check configured frontend from the resolved workspace root. A missing
    // kurogane.toml loads as the defaults
    let packaging_config = match workspace_root.as_deref() {
        None => None,
        Some(root) => match crate::config::PackagingConfig::load(root) {
            Ok(config) => Some((root, config)),
            Err(e) => {
                tui::error("kurogane.toml could not be loaded");
                tui::field("cause", e);
                fail += 1;
                None
            }
        },
    };

    let check_frontend = |root: &std::path::Path, label: &str, path: &std::path::Path| -> bool {
        let anchored = crate::config::anchor_path(root, path);
        if anchored.exists() {
            tui::success(label);
            tui::field("path", tui::format_path(&anchored));
            true
        } else {
            tui::warn(&format!("{label} not found"));
            tui::field("path", tui::format_path(&anchored));
            false
        }
    };

    if let Some((root, config)) = packaging_config {
        if let Some(src) = &config.app.frontend {
            if !check_frontend(root, "Frontend source", src) {
                warn += 1;
            }
        } else {
            tui::info("No frontend source configured in kurogane.toml");
        }

        if let Some(dist) = &config.app.frontend_dist {
            if !check_frontend(root, "Frontend distribution", dist) {
                warn += 1;
            }
        } else {
            tui::info("No frontend-dist configured in kurogane.toml");
        }

        if let Some(install) = &config.app.frontend_install {
            tui::field("frontend-install", install);
        }
        if let Some(run) = &config.app.frontend_run {
            tui::field("frontend-run", run);
        }
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

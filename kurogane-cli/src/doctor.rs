//! Environment and installation diagnostics.
//!
//! This module validates the local CEF installation, runtime discovery,
//! build toolchain and project structure and presents the resulting health
//! report to the user.

use anyhow::Result;
use cargo_metadata::MetadataCommand;
use kurogane_layout::{
    CefSource, cef_path, read_provenance, resolve_cef_for_bundle, validate_cef_runtime,
};

use crate::collector;
use crate::tui;

struct ToolCheck {
    name: &'static str,
    found: bool,
    hint: &'static str,
}

/// What a build needs on the running host, the linker rustc runs; nothing
/// is compiled from C++.
///
/// MSVC's linker is found as rustc and the `cc` crate find it, a developer
/// prompt's `PATH` or else the newest Visual Studio installation, so no
/// developer prompt is needed.
#[cfg(windows)]
fn required_tools() -> Vec<ToolCheck> {
    vec![ToolCheck {
        name: "MSVC linker",
        found: find_msvc_tools::find_tool(tetsu_download::DEFAULT_TARGET, "link.exe").is_some(),
        hint: "Install Visual Studio Build Tools with the C++ workload",
    }]
}

/// What a build needs on the running host, the linker rustc runs; nothing
/// is compiled from C++.
#[cfg(target_os = "macos")]
fn required_tools() -> Vec<ToolCheck> {
    vec![ToolCheck {
        name: "Xcode Command Line Tools (clang)",
        found: probe("clang"),
        hint: "Install Command Line Tools: xcode-select --install",
    }]
}

/// What a build needs on the running host, the linker rustc runs; nothing
/// is compiled from C++.
#[cfg(all(unix, not(target_os = "macos")))]
fn required_tools() -> Vec<ToolCheck> {
    vec![ToolCheck {
        name: "C compiler (cc)",
        found: probe("cc"),
        hint: "Install build-essential or your distro's compiler toolchain",
    }]
}

#[cfg(unix)]
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

    // The CEF the project's application loads, which the commands use too
    let version = crate::install::project_cef_version();

    tui::field("cef", &version);

    match cef_path() {
        Some(cef) => match validate_cef_runtime(&cef) {
            Ok(()) => {
                tui::success("The application loads the runtime CEF_PATH names");
                tui::field("path", tui::format_path(&cef));
            }

            Err(e) => {
                tui::error("CEF_PATH names no usable Chromium runtime");
                tui::field("path", tui::format_path(&cef));
                tui::field("reason", e);

                fail += 1;
            }
        },

        None => match crate::install::installed_cef_dir(&version) {
            Ok(installed) => match validate_cef_runtime(&installed) {
                Ok(()) => {
                    tui::success("The application loads the installed runtime");
                    tui::field("path", tui::format_path(&installed));

                    if let Ok(Some(p)) = read_provenance(&installed) {
                        tui::field("artifact", p.artifact);
                    }
                }

                Err(e) if installed.exists() => {
                    tui::error("Installed Chromium runtime invalid");
                    tui::field("path", tui::format_path(&installed));
                    tui::field("reason", e);
                    tui::info("Run: kurogane install");

                    fail += 1;
                }

                Err(_) => {
                    tui::error("Chromium runtime not installed");
                    tui::field("expected", tui::format_path(&installed));
                    tui::info("Run: kurogane install");

                    fail += 1;
                }
            },

            Err(e) => {
                tui::error("No place to install Chromium");
                tui::field("reason", e);

                fail += 1;
            }
        },
    }

    if let Some(root) = tetsu_download::cef_install_root()
        && let Ok(entries) = std::fs::read_dir(&root)
    {
        let versions: Vec<_> = entries
            .flatten()
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();

        if !versions.is_empty() {
            tui::blank();

            tui::info("Installed versions, shared by every tetsu project");

            for version in versions {
                tui::field("cef", version);
            }
        }
    }

    tui::blank();

    // What `kurogane bundle` packages, with verified provenance
    let packaged = crate::install::installed_cef_dir(&version)
        .and_then(|installed| Ok(resolve_cef_for_bundle(&version, &installed)?));
    match packaged {
        Ok(resolved) => {
            tui::success("bundle packages");
            tui::field("path", tui::format_path(&resolved.root));
            tui::field(
                "source",
                match resolved.source {
                    CefSource::CefPath => "CEF_PATH",
                    CefSource::Installed => "installed",
                },
            );
            tui::field("provenance", resolved.provenance.artifact);
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
        if tool.found {
            tui::success(tool.name);
        } else {
            missing.push(tool);
            fail += 1;
        }
    }

    if !missing.is_empty() {
        tui::error("Build toolchain not found");

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

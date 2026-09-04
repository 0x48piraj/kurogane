//! Contextual environment and project information.
//!
//! Displays the current Kurogane version, platform, runtime environment
//! and project configuration.

use anyhow::Result;
use cargo_metadata::MetadataCommand;

use crate::tui;

pub fn run() -> Result<()> {
    tui::section("Kurogane Info");

    tui::info("Runtime");

    tui::field("version", env!("CARGO_PKG_VERSION"));
    tui::field("cef", env!("KUROGANE_CEF_VERSION"));
    tui::field("os", std::env::consts::OS);
    tui::field("arch", std::env::consts::ARCH);

    tui::blank();

    tui::info("Environment");

    match std::env::var("CEF_PATH") {
        Ok(v) => tui::field("CEF_PATH", v),
        Err(_) => tui::field("CEF_PATH", "not set"),
    }

    tui::blank();

    tui::info("Project");

    match std::env::current_dir() {
        Ok(dir) => tui::field("directory", dir.display()),
        Err(_) => tui::field("directory", "(unknown)"),
    }

    // Resolve workspace root
    let workspace_root = MetadataCommand::new().exec().ok().map(|m| {
        let root = m.workspace_root.into_std_path_buf();
        tui::field("workspace", tui::format_path(&root));
        root
    });

    if workspace_root.is_none() {
        tui::field("workspace", "(not inside a Cargo workspace)");
    }

    let config_path = workspace_root
        .as_ref()
        .map(|r| r.join("kurogane.toml"))
        .unwrap_or_else(|| std::path::PathBuf::from("kurogane.toml"));

    if config_path.exists() {
        tui::field("config", config_path.display());

        let config_dir = config_path.parent().unwrap_or(std::path::Path::new("."));

        match kurogane_layout::PackagingConfig::load(config_dir) {
            Ok(config) => {
                if let Some(name) = &config.app.name {
                    tui::field("name", name);
                }
                if let Some(frontend) = &config.app.frontend {
                    tui::field("frontend", frontend.display());
                } else {
                    tui::field("frontend", "(not configured)");
                }
                if let Some(frontend_dist) = &config.app.frontend_dist {
                    tui::field("frontend-dist", frontend_dist.display());
                } else {
                    tui::field("frontend-dist", "(not configured)");
                }
                if let Some(install) = &config.app.frontend_install {
                    tui::field("frontend-install", install);
                }
                if let Some(run) = &config.app.frontend_run {
                    tui::field("frontend-run", run);
                }
                if let Some(publisher) = &config.app.publisher {
                    tui::field("publisher", publisher);
                }
                if let Some(icon) = &config.app.icon {
                    tui::field("icon", icon.display());
                }
                if !config.bundle.resources.is_empty() {
                    tui::field(
                        "resources",
                        format!("{} declared", config.bundle.resources.len()),
                    );
                }
            }
            Err(err) => {
                tui::warn(&format!("Failed to parse kurogane.toml: {err}"));
            }
        }
    } else {
        tui::field("config", "not found");
        tui::info(
            "Run `kurogane new` to create a project or `kurogane init` to add Kurogane to an existing project",
        );
    }

    tui::blank();

    Ok(())
}

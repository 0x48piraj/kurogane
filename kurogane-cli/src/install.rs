//! The CEF installation.
//!
//! Installs the CEF a project's application loads into tetsu's shared
//! installation, where the application finds it, and names its version and
//! directory for the other commands.

use anyhow::{Context, Result};
use cargo_metadata::Metadata;
use kurogane_layout::validate_cef_runtime;
use std::path::PathBuf;
use tetsu_download::{DEFAULT_TARGET, OsAndArch};

use crate::tui;

/// The CEF version the current project's application loads, the CLI's own
/// outside a project.
pub(crate) fn project_cef_version() -> String {
    cargo_metadata::MetadataCommand::new()
        .exec()
        .ok()
        .and_then(|metadata| cef_version_of(&metadata))
        .unwrap_or_else(|| env!("KUROGANE_CEF_VERSION").to_string())
}

/// The CEF version a resolved project loads, the one its tetsu-sys was
/// generated for (`154.0.33` in `154.4.0+154.0.33`).
pub(crate) fn cef_version_of(metadata: &Metadata) -> Option<String> {
    metadata
        .packages
        .iter()
        .find(|package| package.name == "tetsu-sys")
        .map(|package| package.version.build.as_str().to_owned())
        .filter(|version| !version.is_empty())
}

/// The installation of CEF `version` for this host.
pub(crate) fn installed_cef_dir(version: &str) -> Result<PathBuf> {
    let host = OsAndArch::try_from(DEFAULT_TARGET)?;

    tetsu_download::cef_install_dir(version, &host)
        .context("this user has no local data directory to install Chromium into")
}

pub fn run() -> Result<()> {
    install(&project_cef_version()).map(drop)
}

/// Installs CEF `version` unless a valid installation is in place; returns
/// its directory.
pub(crate) fn install(cef_version: &str) -> Result<PathBuf> {
    tui::section("Kurogane installer");

    let install_dir = installed_cef_dir(cef_version)?;

    if install_dir.exists() {
        match validate_cef_runtime(&install_dir) {
            Ok(()) => {
                tui::success("Chromium engine already installed");
                tui::field("version", cef_version);
                tui::field("path", tui::format_path(&install_dir));
                return Ok(install_dir);
            }
            Err(err) => {
                tui::warn("Existing Chromium runtime is incomplete; reinstalling");
                tui::field("reason", err);
                std::fs::remove_dir_all(&install_dir).with_context(|| {
                    format!("failed to remove directory {}", install_dir.display())
                })?;
            }
        }
    }

    tui::step("Downloading Chromium engine...");
    tui::field("chromium", cef_version);
    tui::field("path", tui::format_path(&install_dir));

    let installed = tetsu_download::install(
        DEFAULT_TARGET,
        cef_version,
        &tetsu_download::default_download_url(),
        true,
    )
    .context("failed to install Chromium")?;

    // Fail rather than leaving an unusable tree for the next run to trip on
    validate_cef_runtime(&installed)
        .map_err(|e| anyhow::anyhow!("installed Chromium runtime is invalid: {e}"))?;

    tui::blank();

    tui::success("Chromium engine installed");
    tui::field("path", tui::format_path(&installed));

    Ok(installed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cli_s_own_workspace_loads_the_cli_s_cef() {
        // The tests run in Kurogane's workspace, whose tetsu-sys the CLI was
        // built against
        assert_eq!(project_cef_version(), env!("KUROGANE_CEF_VERSION"));
    }
}

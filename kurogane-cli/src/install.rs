//! The CEF installation.
//!
//! Installs the CEF a project's application loads into tetsu's shared
//! installation, where the application finds it, and names its version and
//! directory for the other commands.

use anyhow::{Context, Result};
use cargo_metadata::Metadata;
use kurogane_layout::{cef_path, validate_cef_runtime, verify_installation};
use std::path::PathBuf;
use tetsu_download::{DEFAULT_TARGET, OsAndArch};

use crate::tui;

/// The CEF version the current project's application loads, the CLI's own
/// outside a project.
pub(crate) fn project_cef_version() -> String {
    match cargo_metadata::MetadataCommand::new().exec() {
        Ok(metadata) => cef_version_of(&metadata),
        Err(_) => env!("KUROGANE_CEF_VERSION").to_owned(),
    }
}

/// The CEF version a resolved project loads, the one its tetsu-sys was
/// generated for (`154.0.33` in `154.4.0+154.0.33`), else the CLI's own.
pub(crate) fn cef_version_of(metadata: &Metadata) -> String {
    metadata
        .packages
        .iter()
        .find(|package| package.name == "tetsu-sys")
        .map(|package| package.version.build.as_str())
        .filter(|version| !version.is_empty())
        .unwrap_or(env!("KUROGANE_CEF_VERSION"))
        .to_owned()
}

/// The installation of CEF `version` for this host.
pub(crate) fn installed_cef_dir(version: &str) -> Result<PathBuf> {
    let host = OsAndArch::try_from(DEFAULT_TARGET)?;

    tetsu_download::cef_install_dir(version, &host)
        .context("this user has no local data directory to install Chromium into")
}

/// The CEF the application will load, installing CEF `version` when it is
/// missing: the distribution `CEF_PATH` names, else the installation.
///
/// The application finds the same one itself, so nothing is passed to it.
/// A `CEF_PATH` naming no usable runtime is an error, never replaced by the
/// installation.
pub(crate) fn ensure_cef_runtime(version: &str) -> Result<PathBuf> {
    tui::step("Checking Chromium engine");

    if let Some(cef) = cef_path() {
        validate_cef_runtime(&cef)
            .map_err(|err| anyhow::anyhow!("CEF_PATH names no usable Chromium runtime: {err}"))?;

        tui::success("Chromium engine ready");
        tui::field("path", tui::format_path(&cef));
        tui::field("source", "CEF_PATH");

        return Ok(cef);
    }

    let installed = installed_cef_dir(version)?;

    match verify_installation(&installed, version) {
        Ok(_) => {
            tui::success("Chromium engine ready");
            tui::field("path", tui::format_path(&installed));

            Ok(installed)
        }

        Err(err) => {
            if installed.exists() {
                tui::warn("Chromium runtime incomplete or unverified");
                tui::field("reason", err);
            } else {
                tui::warn("Chromium runtime not installed");
            }
            tui::info("Initiating install process...");

            install(version)
        }
    }
}

pub fn run() -> Result<()> {
    install(&project_cef_version()).map(drop)
}

/// Installs CEF `version` unless a verified installation is in place;
/// returns its directory.
pub(crate) fn install(cef_version: &str) -> Result<PathBuf> {
    tui::section("Kurogane installer");

    let install_dir = installed_cef_dir(cef_version)?;

    if install_dir.exists() {
        match verify_installation(&install_dir, cef_version) {
            Ok(_) => {
                tui::success("Chromium engine already installed");
                tui::field("version", cef_version);
                tui::field("path", tui::format_path(&install_dir));
                return Ok(install_dir);
            }
            Err(err) => {
                tui::warn("Existing Chromium runtime is incomplete or unverified; reinstalling");
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
    verify_installation(&installed, cef_version)
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

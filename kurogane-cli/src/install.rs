//! Managed CEF installation.
//!
//! This module downloads the configured CEF distribution, records its
//! provenance and installs it into Kurogane's managed runtime cache.

use anyhow::{Context, Result};
use kurogane_layout::{cef_install_dir, validate_cef_runtime};
use tetsu_download::DEFAULT_TARGET;

use crate::tui;

pub fn run() -> Result<()> {
    tui::section("Kurogane installer");

    let cef_version = env!("KUROGANE_CEF_VERSION").to_string();
    let install_dir = cef_install_dir(&cef_version);

    if install_dir.exists() {
        match validate_cef_runtime(&install_dir) {
            Ok(()) => {
                tui::success("Chromium engine already installed");
                tui::field("version", &cef_version);
                tui::field("path", tui::format_path(&install_dir));
                return Ok(());
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
    tui::field("chromium", &cef_version);
    tui::field("path", tui::format_path(&install_dir));

    // tetsu's shared installation, which plain cargo builds and the
    // applications they start use too
    let installed = tetsu_download::install(
        DEFAULT_TARGET,
        &cef_version,
        &tetsu_download::default_download_url(),
        true,
    )
    .context("failed to install Chromium")?;
    if installed != install_dir {
        anyhow::bail!(
            "tetsu installed Chromium at {}, not at {}, where Kurogane looks",
            installed.display(),
            install_dir.display()
        );
    }

    // Fail rather than leaving an unusable tree for the next run to trip on
    validate_cef_runtime(&install_dir)
        .map_err(|e| anyhow::anyhow!("installed Chromium runtime is invalid: {e}"))?;

    tui::blank();

    tui::success("Chromium engine installed");
    tui::field("path", tui::format_path(&install_dir));

    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn kurogane_and_tetsu_name_one_installation() {
        let version = env!("KUROGANE_CEF_VERSION");
        let os_arch = tetsu_download::OsAndArch::try_from(tetsu_download::DEFAULT_TARGET).unwrap();

        assert_eq!(
            Some(kurogane_layout::install_root()),
            tetsu_download::cef_install_root()
        );
        assert_eq!(
            Some(kurogane_layout::cef_install_dir(version)),
            tetsu_download::cef_install_dir(version, &os_arch),
            "kurogane install, doctor and uninstall see what tetsu installs"
        );
    }
}

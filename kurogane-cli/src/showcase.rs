use anyhow::{Result, bail};
use std::process::Command;

use crate::template;
use crate::tui;

/// The showcase template repository.
const SHOWCASE_TEMPLATE_REPO: &str = "https://github.com/kurogane-rs/kurogane-showcase";

pub fn run(consent: template::Consent) -> Result<()> {
    tui::section("Kurogane Showcase");

    let cache_root = dirs::cache_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not determine cache directory"))?
        .join("kurogane");
    let root = cache_root.join("showcase");

    tui::step("Preparing showcase environment");
    tui::field("path", root.to_string_lossy());

    // Regenerate the showcase project from the cached template
    let acquired = crate::cache::acquire(SHOWCASE_TEMPLATE_REPO)?;
    tui::field("commit", &acquired.commit);
    template::confirm_hooks(&acquired.path, consent)?;
    template::regenerate_project(&acquired.path, "showcase", &root, &[], consent)?;

    tui::step("Launching showcase...");

    let exe = std::env::current_exe()?;

    let status = Command::new(exe).arg("dev").current_dir(root).status()?;

    if !status.success() {
        let code = status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "signal".into());
        bail!("Showcase failed (exit code: {code})");
    }

    Ok(())
}

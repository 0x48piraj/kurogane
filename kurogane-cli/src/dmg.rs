//! Creates macOS disk images.
//!
//! The image holds the signed `.app` beside an `Applications` link, the
//! usual drag-to-install pair. It is staged in a temporary folder, removed
//! on every path, and with `--sign` sealed by the same identity as the app.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::signing::{SignConfig, sign_disk_image};
use crate::tui;

/// Apple's tool for copying bundles: it keeps symbolic links, modes and
/// extended attributes, so the staged app is the one codesign sealed. Never
/// a `ditto` from `PATH`.
const DITTO: &str = "/usr/bin/ditto";

/// The disk image tool, which ships with macOS.
const HDIUTIL: &str = "/usr/bin/hdiutil";

/// What the `Applications` link in the image points to.
const APPLICATIONS: &str = "/Applications";

/// Returns the DMG path for an application name.
fn dmg_path(output_dir: &Path, name: &str) -> PathBuf {
    output_dir.join(format!("{name}.dmg"))
}

/// Stages the image's contents in `staging`: a copy of `app_dir` and an
/// `Applications` link.
fn stage(app_dir: &Path, staging: &Path) -> Result<()> {
    let app_name = app_dir
        .file_name()
        .with_context(|| format!("{} has no file name", app_dir.display()))?;
    let copy = staging.join(app_name);

    let status = Command::new(DITTO)
        .arg(app_dir)
        .arg(&copy)
        .status()
        .with_context(|| format!("failed to run {DITTO}"))?;
    if !status.success() {
        bail!(
            "{DITTO} failed ({status}) copying {} to {}",
            app_dir.display(),
            copy.display()
        );
    }

    let link = staging.join("Applications");
    std::os::unix::fs::symlink(APPLICATIONS, &link)
        .with_context(|| format!("failed to create the link {}", link.display()))
}

/// Creates a compressed DMG holding the application and an `Applications`
/// link, signed and verified with `sign` when given.
pub fn build(
    app_dir: &Path,
    output_dir: &Path,
    name: &str,
    sign: Option<&SignConfig>,
) -> Result<PathBuf> {
    let dmg_path = dmg_path(output_dir, name);

    // A previous image, or whatever else sits under its name, goes first
    if dmg_path.symlink_metadata().is_ok() {
        fs::remove_file(&dmg_path)
            .with_context(|| format!("failed to remove {}", dmg_path.display()))?;
    }

    // Removed when it goes out of scope, on every path below
    let staging = tempfile::Builder::new()
        .prefix("kurogane-dmg-")
        .tempdir()
        .context("failed to create a staging folder for the DMG")?;
    stage(app_dir, staging.path())?;

    let status = Command::new(HDIUTIL)
        .arg("create")
        .arg("-volname")
        .arg(name)
        .arg("-srcfolder")
        .arg(staging.path())
        .arg("-ov")
        .arg("-format")
        .arg("UDZO")
        .arg(&dmg_path)
        .status()
        .with_context(|| format!("failed to run {HDIUTIL}"))?;
    if !status.success() {
        bail!(
            "{HDIUTIL} create failed ({status}) for {}",
            dmg_path.display()
        );
    }

    let staging_path = staging.path().to_path_buf();
    if let Err(error) = staging.close() {
        tui::warn(&format!(
            "could not remove the DMG staging folder {}: {error}",
            staging_path.display()
        ));
    }

    if let Some(config) = sign {
        sign_disk_image(&dmg_path, config)
            .with_context(|| format!("failed to sign {}", dmg_path.display()))?;
        tui::field("signed", format!("{name}.dmg"));
    }

    tui::field("dmg", tui::format_path(&dmg_path));

    Ok(dmg_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dmg_is_named_after_the_app_and_written_beside_it() {
        assert_eq!(
            dmg_path(Path::new("/proj/dist"), "MyApp"),
            Path::new("/proj/dist/MyApp.dmg")
        );
    }

    #[test]
    fn staging_holds_the_app_as_it_is_and_an_applications_link() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("MyApp.app");
        let macos = app.join("Contents/MacOS");
        fs::create_dir_all(&macos).unwrap();
        fs::write(macos.join("myapp"), b"exe").unwrap();
        // A link inside the bundle stays a link
        std::os::unix::fs::symlink("myapp", macos.join("alias")).unwrap();
        let staging = dir.path().join("staging");
        fs::create_dir(&staging).unwrap();

        stage(&app, &staging).unwrap();

        let staged = staging.join("MyApp.app/Contents/MacOS");
        assert_eq!(fs::read(staged.join("myapp")).unwrap(), b"exe");
        assert_eq!(
            fs::read_link(staged.join("alias")).unwrap(),
            Path::new("myapp")
        );
        let link = staging.join("Applications");
        assert_eq!(fs::read_link(&link).unwrap(), Path::new("/Applications"));
        let mut entries: Vec<_> = fs::read_dir(&staging)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        entries.sort();
        assert_eq!(entries, ["Applications", "MyApp.app"]);
    }
}

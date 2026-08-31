//! macOS-only development helpers.

use anyhow::Result;
use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

use crate::tui;

/// Configures a macOS `cargo run` process to discover the CEF runtime.
pub(crate) fn set_env(cmd: &mut Command, cef: &Path) {
    let existing = std::env::var("DYLD_FALLBACK_LIBRARY_PATH").unwrap_or_default();
    cmd.env(
        "DYLD_FALLBACK_LIBRARY_PATH",
        super::prepend_search_path(cef, &existing, ':'),
    );
}

/// Links ANGLE libraries into the directories `cargo run` writes executables to.
///
/// Unbundled Chromium processes load ANGLE from the executable directory.
/// Kurogane exposes the CEF-provided libraries there as symlinks.
pub(crate) fn link_gpu_libraries(cef: &Path, cargo_args: &[OsString]) -> Result<()> {
    use kurogane_layout::link_unbundled_angle_libraries;

    let mut installed = Vec::new();

    for dir in super::probe::executable_dirs(cargo_args)? {
        for name in link_unbundled_angle_libraries(cef, &dir)? {
            if !installed.contains(&name) {
                installed.push(name);
            }
        }
    }

    if !installed.is_empty() {
        tui::step("Linking GPU libraries");
        tui::field("libraries", installed.join(", "));
    }

    Ok(())
}

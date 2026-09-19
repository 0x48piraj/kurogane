//! Shared `libcef_dll_wrapper` builds for macOS.
//!
//! Builds the wrapper once per CEF distribution and reuses the
//! archive across projects.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

use kurogane_layout::{cache_root, read_provenance};

use crate::tui;

/// Archive produced by the `libcef_dll_wrapper` CMake target.
const ARCHIVE: &str = "libcef_dll_wrapper.a";

/// Root of the shared wrapper cache.
fn cache_dir() -> PathBuf {
    cache_root().join("wrapper")
}

/// Returns the CMake architecture name for the host.
fn project_arch() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x86_64"
    }
}

/// Cache key identifying the CEF distribution a wrapper was built from.
fn cache_key(cef: &Path) -> Result<Option<String>> {
    Ok(read_provenance(cef)?.map(|provenance| provenance.artifact))
}

/// Returns the shared wrapper directory for a CEF installation, if keyed.
pub(crate) fn ensure(cef: &Path) -> Result<Option<PathBuf>> {
    let Some(key) = cache_key(cef)? else {
        return Ok(None);
    };

    let dir = cache_dir().join(key);

    // A present archive indicates a complete build
    if dir.join(ARCHIVE).is_file() {
        return Ok(Some(dir));
    }

    build(cef, &dir)?;

    Ok(Some(dir))
}

/// Builds `libcef_dll_wrapper` from a CEF distribution into the cache.
fn build(cef: &Path, dest: &Path) -> Result<()> {
    tui::step("Building CEF wrapper");
    tui::field("cache", tui::format_path(dest));

    // Keep intermediate build artifacts out of the cache entry
    let scratch = dest.join("build");

    if scratch.exists() {
        std::fs::remove_dir_all(&scratch)
            .with_context(|| format!("failed to clear {}", scratch.display()))?;
    }

    std::fs::create_dir_all(&scratch)
        .with_context(|| format!("failed to create {}", scratch.display()))?;

    // Build one archive for both `cef-dll-sys` sandbox configurations
    run(Command::new("cmake")
        .arg("-G")
        .arg("Ninja")
        .arg("-DCMAKE_BUILD_TYPE=RelWithDebInfo")
        .arg(format!("-DPROJECT_ARCH={}", project_arch()))
        .arg("-DUSE_SANDBOX=OFF")
        .arg(cef)
        .current_dir(&scratch))?;

    run(Command::new("cmake")
        .arg("--build")
        .arg(".")
        .arg("--target")
        .arg("libcef_dll_wrapper")
        .current_dir(&scratch))?;

    let built = scratch.join("libcef_dll_wrapper").join(ARCHIVE);

    // Publish only the completed archive
    std::fs::rename(&built, dest.join(ARCHIVE))
        .with_context(|| format!("failed to publish {}", built.display()))?;

    std::fs::remove_dir_all(&scratch)
        .with_context(|| format!("failed to remove {}", scratch.display()))?;

    tui::success("CEF wrapper ready");

    Ok(())
}

/// Runs a build command, surfacing its output only when it fails.
fn run(cmd: &mut Command) -> Result<()> {
    let program = cmd.get_program().to_owned();

    let output = cmd
        .output()
        .with_context(|| format!("failed to run {program:?}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "{program:?} exited with {}\n\n{}",
            output.status,
            stderr.trim()
        );
    }

    Ok(())
}

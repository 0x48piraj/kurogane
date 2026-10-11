//! Build-time setup for Kurogane applications.
//!
//! An application's `build.rs` calls [`build`] from its `main`, with
//! `kurogane-build` among its build dependencies.

/// Prepares the package's executables for Chromium on the target platform.
///
/// On Windows with the MSVC toolchain it embeds the application manifest CEF's
/// own executables carry, through [`tanso_build::embed_windows_manifest`].
/// Without it Windows tells Chromium it runs on Windows 8. Elsewhere it does
/// nothing.
///
/// # Panics
///
/// Panics outside a build script, where Cargo sets no `OUT_DIR`, or when the
/// manifest cannot be written there.
pub fn build() {
    tanso_build::embed_windows_manifest();
}

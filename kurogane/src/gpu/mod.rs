//! GPU backend selection.
//!
//! Called once during CEF command-line processing before any browser is created.

mod backend;
mod detection;

#[cfg(target_os = "linux")]
pub(super) mod linux;

#[cfg(target_os = "windows")]
pub(super) mod windows;

#[cfg(target_os = "macos")]
pub(super) mod macos;

pub use backend::GpuMode;

use cef::CommandLine;

/// Apply Chromium command-line flags for the configured GPU mode
pub(crate) fn apply_gpu_flags(
    cmd: &mut CommandLine,
    mode: GpuMode,
) {
    backend::apply(cmd, mode);
}

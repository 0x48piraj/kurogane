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

use crate::chromium_flags::ChromiumFlags;
use crate::spec::SandboxMode;

/// Apply Chromium command-line flags for the configured GPU mode
///
/// The sandbox policy decides where GPU work may run, see
/// [`SandboxMode::Chromium`].
pub(crate) fn apply_gpu_flags(flags: &mut ChromiumFlags, mode: GpuMode, sandbox: SandboxMode) {
    backend::apply_gpu_flags(flags, mode, sandbox);
}

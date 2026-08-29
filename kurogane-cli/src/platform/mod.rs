//! Platform-specific development helpers for the Kurogane CLI.

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
pub(crate) use macos::link_unbundled_gpu_libraries;

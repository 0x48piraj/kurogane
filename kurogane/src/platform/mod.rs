//! Platform-specific initialization.

#[cfg(target_os = "macos")]
pub(crate) mod macos;

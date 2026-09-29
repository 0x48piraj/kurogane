//! Platform-specific code.

#[cfg(target_os = "macos")]
pub(crate) mod macos;

pub(crate) mod embed;

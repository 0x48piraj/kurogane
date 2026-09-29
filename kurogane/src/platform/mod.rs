//! Platform-specific code.

#[cfg(target_os = "macos")]
pub(crate) mod macos;

#[cfg(any(target_os = "windows", target_os = "macos"))]
pub(crate) mod embed;

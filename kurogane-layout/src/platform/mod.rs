//! Platform directories and names.

use std::path::PathBuf;

/// Returns the user's local data directory, else the temporary directory.
pub fn data_local_dir() -> PathBuf {
    dirs::data_local_dir().unwrap_or_else(std::env::temp_dir)
}

/// macOS CEF framework directory name.
#[cfg(target_os = "macos")]
pub const MACOS_FRAMEWORK: &str = "Chromium Embedded Framework.framework";

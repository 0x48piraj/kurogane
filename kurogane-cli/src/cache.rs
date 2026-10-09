//! Kurogane's cache directories.
//!
//! Everything here can be removed and rebuilt; application profiles live in
//! the local data directory instead.

use std::path::PathBuf;

/// Returns Kurogane's folder in the user's cache directory.
pub fn cache_root() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("kurogane")
}

/// Returns the folder of the Linux build tools `kurogane bundle` downloads.
pub fn tools_dir() -> PathBuf {
    cache_root().join("tools")
}

/// Returns the folder of git template snapshots.
pub fn templates_dir() -> PathBuf {
    cache_root().join("templates")
}

/// Returns the folder of the showcase project `kurogane showcase` runs.
pub fn showcase_dir() -> PathBuf {
    cache_root().join("showcase")
}

//! Platform-specific application and cache directories.
//!
//! This module provides small cross-platform wrappers around platform data
//! directories used by Kurogane for managed runtimes and runtime caches.

use std::path::PathBuf;

pub fn data_local_dir() -> PathBuf {
    dirs::data_local_dir().unwrap_or_else(std::env::temp_dir)
}

pub fn cache_dir() -> PathBuf {
    dirs::cache_dir().unwrap_or_else(std::env::temp_dir)
}

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
pub use macos::link_unbundled_angle_libraries;

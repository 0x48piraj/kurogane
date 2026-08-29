//! macOS GPU flags configuration.
//!
//! macOS handles GPU acceleration through Metal and Core Animation
//! without requiring Chromium command-line overrides.

use crate::chromium_flags::ChromiumFlags;

pub(super) fn apply_hardware(_flags: &mut ChromiumFlags) {
    // No platform-specific GPU flags needed on macOS
}

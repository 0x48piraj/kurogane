//! macOS sandbox flags configuration.
//!
//! macOS enforces sandboxing through the App Sandbox entitlements and the
//! hardened runtime not through Chromium command-line overrides.

use crate::chromium_flags::ChromiumFlags;

pub(super) fn apply_sandbox_flags(_flags: &mut ChromiumFlags) {
    // No platform-specific sandbox flags needed on macOS
}

//! macOS credential storage configuration.

use crate::chromium_flags::ChromiumFlags;

/// Replaces the Keychain with Chromium's in-process mock store.
///
/// Keychain access is granted to a specific code identity. An unsigned binary
/// has none that survives a rebuild, so every run raises a fresh authorization
/// prompt; denying it leaves Chromium unable to encrypt at all.
pub(super) fn apply_basic(flags: &mut ChromiumFlags) {
    flags.set("use-mock-keychain");
}

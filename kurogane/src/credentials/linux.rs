//! Linux credential storage configuration.

use crate::chromium_flags::ChromiumFlags;

/// Selects Chromium's built-in store over kwallet and gnome-keyring.
///
/// Headless hosts, containers and CI machines commonly run no keyring daemon,
/// where the desktop stores either prompt or fail outright.
pub(super) fn apply_basic(flags: &mut ChromiumFlags) {
    flags.set_with_value("password-store", "basic");
}

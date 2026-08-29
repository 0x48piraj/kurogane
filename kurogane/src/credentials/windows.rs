//! Windows credential storage configuration.
//!
//! DPAPI is keyed to the logged-in user and never prompts, so Chromium offers
//! no switch to bypass it.

use crate::chromium_flags::ChromiumFlags;

pub(super) fn apply_basic(_flags: &mut ChromiumFlags) {
    // No platform-specific credential flags needed on Windows.
}

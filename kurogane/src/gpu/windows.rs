//! Windows GPU flags configuration.
//!
//! Chromium runs GPU work in a GPU process of its own, as it does by default.
//! Inside the browser process (`--in-process-gpu`) the pages of Chromium's own
//! windows went unpainted on some machines.

use crate::chromium_flags::ChromiumFlags;

pub(super) fn apply_hardware(_flags: &mut ChromiumFlags) {
    // No platform-specific GPU flags needed on Windows
}

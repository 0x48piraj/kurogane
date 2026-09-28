//! Windows GPU flags configuration.

use crate::chromium_flags::ChromiumFlags;
use crate::spec::SandboxMode;

pub(super) fn apply_hardware(flags: &mut ChromiumFlags, sandbox: SandboxMode) {
    // Sandboxed Chromium uses a separate GPU process
    if sandbox == SandboxMode::Chromium {
        return;
    }

    // Avoid restarting the GPU process after device loss
    flags.set("in-process-gpu");
}

use crate::chromium_flags::ChromiumFlags;
use crate::error::RuntimeError;

pub(crate) fn apply_sandbox_flags(flags: &mut ChromiumFlags) {
    // Sandbox disable
    flags.set("no-sandbox");
    flags.set("disable-gpu-sandbox");
}

/// Windows passes sandbox information only through CEF's launch model
/// which Kurogane does not use yet.
pub(crate) fn preflight() -> Result<(), RuntimeError> {
    Err(RuntimeError::SandboxUnsupported {
        reason: "the Chromium sandbox is not supported on Windows yet".into(),
    })
}

use crate::chromium_flags::ChromiumFlags;

pub(crate) fn apply_sandbox_flags(flags: &mut ChromiumFlags) {
    // Sandbox disable
    flags.set("no-sandbox");
    flags.set("disable-gpu-sandbox");
}

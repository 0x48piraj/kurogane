use crate::chromium_flags::ChromiumFlags;

pub(crate) fn apply_sandbox_flags(flags: &mut ChromiumFlags) {
    flags.set("disable-setuid-sandbox");
}

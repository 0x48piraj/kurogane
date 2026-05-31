use cef::*;

pub(crate) fn apply_sandbox_flags(
    cmd: &mut CommandLine,
) {
    // Sandbox disable
    cmd.append_switch(Some(&CefString::from("no-sandbox")));
    cmd.append_switch(Some(&CefString::from("disable-gpu-sandbox")));
}

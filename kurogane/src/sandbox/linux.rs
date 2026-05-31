use cef::*;

pub(crate) fn apply_sandbox_flags(
    cmd: &mut CommandLine,
) {
    cmd.append_switch(Some(&CefString::from("disable-setuid-sandbox")));
}

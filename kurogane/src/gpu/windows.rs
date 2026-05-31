//! Windows GPU flags configuration.

use cef::*;

pub(super) fn apply_platform_flags(
    cmd: &mut CommandLine,
) {
    // Sandbox disable
    cmd.append_switch(Some(&CefString::from("no-sandbox")));
    cmd.append_switch(Some(&CefString::from("disable-gpu-sandbox")));
}

pub(super) fn apply_hardware(
    cmd: &mut CommandLine,
) {
    // Run GPU work inside the browser process rather than in a child.
    //
    // On Windows + NVIDIA, the sandboxed GPU subprocess cannot survive a D3D
    // context reset (Chromium bug workaround: exit_on_context_lost).
    // After 3 crashes Chromium falls back to software.
    // Keeping GPU in-process avoids the subprocess entirely and
    // gives stable hardware acceleration.
    cmd.append_switch(Some(&CefString::from("in-process-gpu")));
}

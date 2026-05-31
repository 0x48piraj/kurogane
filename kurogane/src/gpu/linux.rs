//! Linux GPU flags configuration.

use cef::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LinuxGpuDriver {
    NvidiaWayland,
    NvidiaX11,
    MesaWayland,
    MesaX11,
}

pub(super) fn apply_platform_flags(
    cmd: &mut CommandLine,
) {
    cmd.append_switch(Some(&CefString::from("disable-setuid-sandbox")));
}

pub(super) fn apply_hardware(
    cmd: &mut CommandLine,
) {
    match detect_driver() {
        LinuxGpuDriver::NvidiaWayland => {
            // NVIDIA's EGL + Wayland path seems to be unstable
            // Force X11 via the ozone platform selector
            cmd.append_switch_with_value(
                Some(&CefString::from("ozone-platform")),
                Some(&CefString::from("x11")),
            );
        }

        LinuxGpuDriver::NvidiaX11 | LinuxGpuDriver::MesaWayland | LinuxGpuDriver::MesaX11 => {
            // Seemingly stable stack: AMD/Intel, or X11, or Mesa + Wayland (let Chromium do it's thing)
            cmd.append_switch_with_value(
                Some(&CefString::from("ozone-platform-hint")),
                Some(&CefString::from("auto")),
            );
        }
    }
}

fn detect_driver() -> LinuxGpuDriver {
    let nvidia = detect_nvidia();

    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();

    match (nvidia, wayland) {
        (true, true) => LinuxGpuDriver::NvidiaWayland,
        (true, false) => LinuxGpuDriver::NvidiaX11,
        (false, true) => LinuxGpuDriver::MesaWayland,
        (false, false) => LinuxGpuDriver::MesaX11,
    }
}

fn detect_nvidia() -> bool {
    // Primary: PCI device list (vendor ID 10de = NVIDIA)
    if let Ok(s) = std::fs::read_to_string("/proc/bus/pci/devices") {
        if s.contains("10de") {
            return true;
        }
    }

    // Fallback: check whether the nvidia kernel module is loaded
    if let Ok(s) = std::fs::read_to_string("/proc/modules") {
        if s.lines().any(|l| l.starts_with("nvidia ")) {
            return true;
        }
    }

    false
}

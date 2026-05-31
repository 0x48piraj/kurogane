//! Linux GPU flags configuration.

use cef::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GpuVendor {
    Nvidia,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DisplayServer {
    Wayland,
    X11,
}

pub(super) fn apply_hardware(
    cmd: &mut CommandLine,
) {
    let vendor = detect_gpu_vendor();
    let display = detect_display_server();

    match (vendor, display) {
        (GpuVendor::Nvidia, DisplayServer::Wayland) => {
            // NVIDIA's EGL + Wayland path seems to be unstable
            // Force X11 via the ozone platform selector
            cmd.append_switch_with_value(
                Some(&CefString::from("ozone-platform")),
                Some(&CefString::from("x11")),
            );
        }

        _ => {
            // Seemingly stable stack: AMD/Intel, or X11, or Mesa + Wayland (let Chromium do it's thing)
            cmd.append_switch_with_value(
                Some(&CefString::from("ozone-platform-hint")),
                Some(&CefString::from("auto")),
            );
        }
    }
}

fn detect_display_server() -> DisplayServer {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        DisplayServer::Wayland
    } else {
        DisplayServer::X11
    }
}

fn detect_gpu_vendor() -> bool {
    // Primary: PCI device list (vendor ID 10de = NVIDIA)
    if let Ok(s) = std::fs::read_to_string("/proc/bus/pci/devices") {
        if s.contains("10de") {
            return GpuVendor::Nvidia;
        }
    }

    // Fallback: check whether the nvidia kernel module is loaded
    if let Ok(s) = std::fs::read_to_string("/proc/modules") {
        if s.lines().any(|l| l.starts_with("nvidia ")) {
            return GpuVendor::Nvidia;
        }
    }

    GpuVendor::Other
}

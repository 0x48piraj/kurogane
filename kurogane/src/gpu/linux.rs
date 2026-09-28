//! Linux GPU flags configuration.

use crate::chromium_flags::ChromiumFlags;
use crate::spec::SandboxMode;

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

pub(super) fn apply_hardware(flags: &mut ChromiumFlags, _sandbox: SandboxMode) {
    let vendor = detect_gpu_vendor();
    let display = detect_display_server();

    match (vendor, display) {
        (GpuVendor::Nvidia, DisplayServer::Wayland) => {
            // NVIDIA's EGL + Wayland path seems to be unstable
            // Force X11 via the ozone platform selector
            flags.set_with_value("ozone-platform", "x11");
        }

        _ => {
            // Seemingly stable stack: AMD/Intel, or X11, or Mesa + Wayland (let Chromium do it's thing)
            flags.set_with_value("ozone-platform-hint", "auto");
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

/// Vendor IDs of the PCI devices present.
pub(super) fn pci_vendors() -> Vec<u16> {
    std::fs::read_to_string("/proc/bus/pci/devices")
        .map(|devices| parse_pci_vendors(&devices))
        .unwrap_or_default()
}

/// Parses the vendor ID from each line of `/proc/bus/pci/devices`.
///
/// The vendor ID is the first four hex digits of the second field.
fn parse_pci_vendors(devices: &str) -> Vec<u16> {
    devices
        .lines()
        .filter_map(|line| line.split_whitespace().nth(1)?.get(..4))
        .filter_map(|vendor| u16::from_str_radix(vendor, 16).ok())
        .collect()
}

fn detect_gpu_vendor() -> GpuVendor {
    // Primary: PCI device list (vendor ID 10de = NVIDIA)
    if pci_vendors().contains(&0x10de) {
        return GpuVendor::Nvidia;
    }

    // Fallback: check whether the nvidia kernel module is loaded
    if let Ok(s) = std::fs::read_to_string("/proc/modules")
        && s.lines().any(|l| l.starts_with("nvidia "))
    {
        return GpuVendor::Nvidia;
    }

    GpuVendor::Other
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vendors_are_read_from_the_id_column_only() {
        // An Intel and an AMD device whose addresses contain "1af4" (virtio)
        // and "10de" (NVIDIA)
        let devices = "0010\t80869a49\t9f\t00000000a1af4004\t0\ti915\n\
                       0300\t100273bf\t7e\t00000000f10de00c\t0\tamdgpu\n";

        assert_eq!(parse_pci_vendors(devices), [0x8086, 0x1002]);
    }
}

//! Environment detection for GPU backend selection.

#[derive(Debug)]
pub(crate) struct RenderingEnvironment {
    /// True when the GPU appears to be a virtual device (VirtualBox, VMware, QEMU etc.)
    pub is_virtual_gpu: bool,
}

impl RenderingEnvironment {
    /// Detect the current GPU environment
    pub(crate) fn detect() -> Self {
        Self {
            is_virtual_gpu: detect_virtual_gpu(), // TODO: optimize for wsl
        }
    }
}

fn detect_virtual_gpu() -> bool {
    #[cfg(target_os = "linux")]
    {
        if let Ok(s) = std::fs::read_to_string("/proc/bus/pci/devices") {
            // VirtualBox: 80ee, VMware: 15ad, QEMU/Virtio: 1af4, Red Hat VirtIO: 1b36
            const VIRTUAL_VENDORS: &[&str] = &["80ee", "15ad", "1af4", "1b36"];
            if VIRTUAL_VENDORS.iter().any(|v| s.contains(v)) {
                return true;
            }
        }
        return false;
    }

    #[cfg(not(target_os = "linux"))]
    false
}

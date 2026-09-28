//! Environment detection for GPU backend selection.

#[derive(Debug)]
pub(crate) struct RenderingEnvironment {
    /// True when the GPU appears to be a virtual device
    pub virtualization: bool,
}

impl RenderingEnvironment {
    /// Detect the current GPU environment
    pub(crate) fn detect() -> Self {
        Self {
            virtualization: detect_virtual_gpu(), // TODO: optimize for wsl
        }
    }
}

fn detect_virtual_gpu() -> bool {
    #[cfg(target_os = "linux")]
    {
        // VirtualBox (80ee), VMware (15ad), QEMU/Virtio (1af4), Red Hat VirtIO (1b36)
        const VIRTUAL_VENDORS: &[u16] = &[0x80ee, 0x15ad, 0x1af4, 0x1b36];
        super::linux::pci_vendors()
            .iter()
            .any(|vendor| VIRTUAL_VENDORS.contains(vendor))
    }

    #[cfg(not(target_os = "linux"))]
    false
}

//! D3D12 import of a ring's slots.

use std::os::windows::io::AsRawHandle;

use kurogane::{GpuAdapter, GpuFrame};
use wgpu::hal::api::Dx12;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Graphics::Direct3D12::ID3D12Resource;

use crate::ImportError;

/// Returns the GPU of `adapter`.
pub(crate) fn adapter_gpu(adapter: &wgpu::Adapter) -> Option<GpuAdapter> {
    // SAFETY: the guard lives through the read; GetDesc1 takes no arguments
    unsafe {
        let hal = adapter.as_hal::<Dx12>()?;
        let desc = hal.raw_adapter().GetDesc1().ok()?;
        Some(GpuAdapter::from_luid(
            desc.AdapterLuid.HighPart,
            desc.AdapterLuid.LowPart,
        ))
    }
}

/// Returns the GPU of a D3D12 `device`, None for another backend.
pub(crate) fn device_gpu(device: &wgpu::Device) -> Option<Option<GpuAdapter>> {
    // SAFETY: the guard lives through the read; GetAdapterLuid takes no arguments
    unsafe {
        let hal = device.as_hal::<Dx12>()?;
        let luid = hal.raw_device().GetAdapterLuid();
        Some(Some(GpuAdapter::from_luid(luid.HighPart, luid.LowPart)))
    }
}

/// Takes a texture for the queue, which needs no barrier. A shared texture
/// decays to the common state between uses.
pub(crate) fn acquire(_device: &wgpu::Device, _queue: &wgpu::Queue, _texture: &wgpu::Texture) {}

/// Gives a texture back, which needs no barrier as [`acquire`] says.
pub(crate) fn release(_device: &wgpu::Device, _queue: &wgpu::Queue, _texture: &wgpu::Texture) {}

/// Imports the slot of `lease` as a texture of `desc`.
pub(crate) fn import(
    device: &wgpu::Device,
    lease: &GpuFrame,
    desc: &wgpu::TextureDescriptor<'_>,
) -> Result<wgpu::Texture, ImportError> {
    let handle = HANDLE(lease.handle().as_raw_handle());
    // SAFETY: handle is a live NT handle of a shared texture; the guard ends
    // before the texture is wrapped
    let resource = unsafe {
        let hal = device
            .as_hal::<Dx12>()
            .ok_or(ImportError::UnsupportedBackend)?;
        let mut resource: Option<ID3D12Resource> = None;
        hal.raw_device()
            .OpenSharedHandle(handle, &raw mut resource)
            .map_err(|e| ImportError::Slot(e.to_string()))?;
        resource.ok_or_else(|| ImportError::Slot("it opened as nothing".to_owned()))?
    };
    // SAFETY: the resource is a 2D texture of desc's size and format, on this
    // device, written in full by the ring before any lease
    Ok(unsafe {
        let hal_texture = wgpu::hal::dx12::Device::texture_from_raw(
            resource,
            desc.format,
            desc.dimension,
            desc.size,
            desc.mip_level_count,
            desc.sample_count,
        );
        device.create_texture_from_hal::<Dx12>(hal_texture, desc, wgpu::TextureUses::UNINITIALIZED)
    })
}

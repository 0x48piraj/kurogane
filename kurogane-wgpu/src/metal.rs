//! Metal import of a ring's slots.

use kurogane::{GpuAdapter, GpuFrame};
use objc2_metal::{
    MTLCopyAllDevices, MTLDevice, MTLPixelFormat, MTLStorageMode, MTLTextureDescriptor,
    MTLTextureType, MTLTextureUsage,
};
use wgpu::hal::api::Metal;

use crate::ImportError;

/// Returns whether `adapter` is the GPU `gpu`, by its name since wgpu keeps the
/// adapter's Metal device to itself.
pub(crate) fn is_gpu(adapter: &wgpu::Adapter, gpu: GpuAdapter) -> bool {
    let name = adapter.get_info().name;
    MTLCopyAllDevices()
        .iter()
        .any(|device| device.registryID() == gpu.registry_id() && device.name().to_string() == name)
}

/// Returns the GPU of a Metal `device`, None for another backend.
pub(crate) fn device_gpu(device: &wgpu::Device) -> Option<Option<GpuAdapter>> {
    // SAFETY: the guard lives through the read
    unsafe {
        let hal = device.as_hal::<Metal>()?;
        Some(Some(GpuAdapter::from_registry_id(
            hal.raw_device().registryID(),
        )))
    }
}

/// Takes a texture for the queue, which needs nothing. The ring's wait orders
/// its writes before the lease.
pub(crate) fn acquire(_device: &wgpu::Device, _queue: &wgpu::Queue, _texture: &wgpu::Texture) {}

/// Gives a texture back, which needs nothing as [`acquire`] says.
pub(crate) fn release(_device: &wgpu::Device, _queue: &wgpu::Queue, _texture: &wgpu::Texture) {}

/// Imports the slot of `lease` as a texture of `desc`.
pub(crate) fn import(
    device: &wgpu::Device,
    lease: &GpuFrame,
    desc: &wgpu::TextureDescriptor<'_>,
) -> Result<wgpu::Texture, ImportError> {
    let format = match desc.format {
        wgpu::TextureFormat::Rgba8Unorm => MTLPixelFormat::RGBA8Unorm,
        _ => MTLPixelFormat::BGRA8Unorm,
    };
    // SAFETY: the sizes are the surface's; the guard ends before the texture
    // is wrapped
    let texture = unsafe {
        let hal = device
            .as_hal::<Metal>()
            .ok_or(ImportError::UnsupportedBackend)?;
        let raw = hal.raw_device();
        let descriptor =
            MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
                format,
                desc.size.width as usize,
                desc.size.height as usize,
                false,
            );
        descriptor.setUsage(MTLTextureUsage::ShaderRead | MTLTextureUsage::PixelFormatView);
        descriptor.setStorageMode(if raw.hasUnifiedMemory() {
            MTLStorageMode::Shared
        } else {
            MTLStorageMode::Managed
        });
        raw.newTextureWithDescriptor_iosurface_plane(&descriptor, lease.io_surface(), 0)
            .ok_or_else(|| ImportError::Slot("its surface did not wrap".to_owned()))?
    };
    // SAFETY: the texture is of this device, of desc's size and format, written
    // in full by the ring before any lease
    Ok(unsafe {
        let hal_texture = wgpu::hal::metal::Device::texture_from_raw(
            texture,
            desc.format,
            MTLTextureType::Type2D,
            1,
            1,
            wgpu::hal::CopyExtent {
                width: desc.size.width,
                height: desc.size.height,
                depth: 1,
            },
            None,
        );
        device.create_texture_from_hal::<Metal>(hal_texture, desc, wgpu::TextureUses::RESOURCE)
    })
}

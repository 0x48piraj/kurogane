//! Vulkan import of a ring's slots.
//!
//! The ring writes a slot on another device between leases. Each lease takes
//! the image from `QUEUE_FAMILY_EXTERNAL` and gives it back, what the
//! specification asks of memory another device writes.

use ash::vk;
use kurogane::{GpuAdapter, GpuFrame};
use wgpu::hal::api::Vulkan;

use crate::ImportError;

/// Features a device needs to import a slot.
#[cfg(windows)]
pub(crate) const NEEDED: wgpu::Features = wgpu::Features::VULKAN_EXTERNAL_MEMORY_WIN32;
/// Features a device needs to import a slot.
#[cfg(target_os = "linux")]
pub(crate) const NEEDED: wgpu::Features = wgpu::Features::VULKAN_EXTERNAL_MEMORY_DMA_BUF;

/// Returns the GPU of a physical device.
#[cfg(windows)]
fn physical_gpu(instance: &ash::Instance, physical: vk::PhysicalDevice) -> Option<GpuAdapter> {
    let mut id = vk::PhysicalDeviceIDProperties::default();
    let mut properties = vk::PhysicalDeviceProperties2::default().push_next(&mut id);
    // SAFETY: physical comes from instance; properties lives through the call
    unsafe { instance.get_physical_device_properties2(physical, &mut properties) };
    if id.device_luid_valid == 0 {
        return None;
    }
    let [a, b, c, d, e, f, g, h] = id.device_luid;
    Some(GpuAdapter::from_luid(
        i32::from_le_bytes([e, f, g, h]),
        u32::from_le_bytes([a, b, c, d]),
    ))
}

/// Returns the GPU of a physical device.
#[cfg(target_os = "linux")]
fn physical_gpu(instance: &ash::Instance, physical: vk::PhysicalDevice) -> Option<GpuAdapter> {
    // SAFETY: physical comes from instance
    let extensions = unsafe { instance.enumerate_device_extension_properties(physical) }.ok()?;
    let has_drm = extensions.iter().any(|extension| {
        extension.extension_name_as_c_str() == Ok(ash::ext::physical_device_drm::NAME)
    });
    if !has_drm {
        return None;
    }
    let mut drm = vk::PhysicalDeviceDrmPropertiesEXT::default();
    let mut properties = vk::PhysicalDeviceProperties2::default().push_next(&mut drm);
    // SAFETY: physical comes from instance; properties lives through the call
    unsafe { instance.get_physical_device_properties2(physical, &mut properties) };
    (drm.has_render != 0)
        .then(|| GpuAdapter::from_render_node(drm.render_major as u32, drm.render_minor as u32))
}

/// Returns the GPU of `adapter`.
pub(crate) fn adapter_gpu(adapter: &wgpu::Adapter) -> Option<GpuAdapter> {
    // SAFETY: the guard lives through the read
    unsafe {
        let hal = adapter.as_hal::<Vulkan>()?;
        physical_gpu(
            hal.shared_instance().raw_instance(),
            hal.raw_physical_device(),
        )
    }
}

/// Returns the GPU of a Vulkan `device`, None for another backend.
pub(crate) fn device_gpu(device: &wgpu::Device) -> Option<Option<GpuAdapter>> {
    // SAFETY: the guard lives through the read
    unsafe {
        let hal = device.as_hal::<Vulkan>()?;
        Some(physical_gpu(
            hal.shared_instance().raw_instance(),
            hal.raw_physical_device(),
        ))
    }
}

/// Imports the slot of `lease` with the device's own helper.
#[cfg(windows)]
unsafe fn import_slot(
    hal: &wgpu::hal::vulkan::Device,
    lease: &GpuFrame,
    desc: &wgpu::hal::TextureDescriptor<'_>,
) -> Result<wgpu::hal::vulkan::Texture, ImportError> {
    use std::os::windows::io::AsRawHandle;

    let handle = windows::Win32::Foundation::HANDLE(lease.handle().as_raw_handle());
    // SAFETY: the caller's promise; handle is a live NT handle of a D3D11
    // texture of desc's size and format
    unsafe { hal.texture_from_d3d11_shared_handle(handle, desc) }
        .map_err(|e| ImportError::Slot(e.to_string()))
}

/// Imports the slot of `lease` with the device's own helper.
#[cfg(target_os = "linux")]
unsafe fn import_slot(
    hal: &wgpu::hal::vulkan::Device,
    lease: &GpuFrame,
    desc: &wgpu::hal::TextureDescriptor<'_>,
) -> Result<wgpu::hal::vulkan::Texture, ImportError> {
    let plane = lease.dmabuf();
    let fd = plane
        .fd
        .try_clone_to_owned()
        .map_err(|e| ImportError::Slot(e.to_string()))?;
    // SAFETY: the caller's promise; fd is a duplicate of a dmabuf of desc's size
    // and format, laid out as the ring reports
    unsafe {
        hal.texture_from_dmabuf_fd(
            fd,
            desc,
            lease.modifier(),
            u64::from(plane.stride),
            plane.offset,
        )
    }
    .map_err(|e| ImportError::Slot(e.to_string()))
}

/// Imports the slot of `lease` as a texture of `desc`.
pub(crate) fn import(
    device: &wgpu::Device,
    lease: &GpuFrame,
    desc: &wgpu::TextureDescriptor<'_>,
) -> Result<wgpu::Texture, ImportError> {
    let hal_desc = wgpu::hal::TextureDescriptor {
        label: desc.label,
        size: desc.size,
        mip_level_count: desc.mip_level_count,
        sample_count: desc.sample_count,
        dimension: desc.dimension,
        format: desc.format,
        usage: wgpu::TextureUses::RESOURCE | wgpu::TextureUses::COPY_SRC,
        memory_flags: wgpu::hal::MemoryFlags::empty(),
        view_formats: desc.view_formats.to_vec(),
    };
    // SAFETY: the slot is of this device's GPU; the guard ends before the
    // texture is wrapped. The first acquire brings the image to the state named
    // here
    unsafe {
        let hal_texture = {
            let hal = device
                .as_hal::<Vulkan>()
                .ok_or(ImportError::UnsupportedBackend)?;
            import_slot(&hal, lease, &hal_desc)?
        };
        Ok(
            device.create_texture_from_hal::<Vulkan>(
                hal_texture,
                desc,
                wgpu::TextureUses::RESOURCE,
            ),
        )
    }
}

/// Records a transfer of `texture` between the queue and the outside, moving
/// it between `GENERAL` and the layout a sampled image has, on `queue`.
fn transfer(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture, acquire: bool) {
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    // SAFETY: the guards live through the recording, which holds one barrier on
    // an image of this device
    unsafe {
        let hal_device = device.as_hal::<Vulkan>();
        let hal_texture = texture.as_hal::<Vulkan>();
        let (Some(hal_device), Some(hal_texture)) = (hal_device, hal_texture) else {
            return;
        };
        let family = hal_device.queue_family_index();
        let (source, target, from, to) = if acquire {
            (
                vk::QUEUE_FAMILY_EXTERNAL,
                family,
                vk::ImageLayout::GENERAL,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            )
        } else {
            (
                family,
                vk::QUEUE_FAMILY_EXTERNAL,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                vk::ImageLayout::GENERAL,
            )
        };
        let range = vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .level_count(1)
            .layer_count(1);
        let barrier = vk::ImageMemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::MEMORY_WRITE | vk::AccessFlags::MEMORY_READ)
            .dst_access_mask(vk::AccessFlags::MEMORY_WRITE | vk::AccessFlags::MEMORY_READ)
            .old_layout(from)
            .new_layout(to)
            .src_queue_family_index(source)
            .dst_queue_family_index(target)
            .image(hal_texture.raw_handle())
            .subresource_range(range);
        encoder.as_hal_mut::<Vulkan, _, _>(|raw| {
            if let Some(raw) = raw {
                hal_device.raw_device().cmd_pipeline_barrier(
                    raw.raw_handle(),
                    vk::PipelineStageFlags::ALL_COMMANDS,
                    vk::PipelineStageFlags::ALL_COMMANDS,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &[barrier],
                );
            }
        });
    }
    queue.submit([encoder.finish()]);
}

/// Takes `texture` from the outside for the queue's reads.
pub(crate) fn acquire(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture) {
    transfer(device, queue, texture, true);
}

/// Gives `texture` back to the outside, from whatever state the host left it in.
pub(crate) fn release(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture) {
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.transition_resources(
        std::iter::empty(),
        std::iter::once(wgpu::TextureTransition {
            texture,
            selector: None,
            state: wgpu::TextureUses::RESOURCE,
        }),
    );
    queue.submit([encoder.finish()]);
    transfer(device, queue, texture, false);
}

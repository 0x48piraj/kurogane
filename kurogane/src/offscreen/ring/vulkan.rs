//! Linux private device of the frame ring.
//!
//! A Vulkan device imports CEF's dmabufs, waits on the fences Chromium left in
//! them and copies into dmabufs Kurogane exports. The buffers belong to the
//! outside between frames. Each copy takes them from the foreign queue family
//! and gives them back. Everything but the dmabufs stays on CEF's UI thread, the
//! one thread that paints.

use std::cell::{Cell, RefCell};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
use std::rc::{Rc, Weak};
use std::sync::OnceLock;

use ash::vk;
use tracing::debug;

use super::writer::{Copy, FrameDevice, OpensFrames};
use super::{GpuAdapter, GpuChoice, Shape};
use crate::offscreen::frame::{PixelFrame, PixelRect, SharedTextureFrame, TextureFormat};

/// Textures of CEF's a browser keeps open, more than its pool hands out at a
/// steady size.
const KEPT_OPEN: usize = 4;

/// Longest wait on the fence in nanoseconds before the device's state is read
/// again. A lost device must not hold CEF's UI thread for good.
const WAIT_SLICE: u64 = 100_000_000;

/// `DRM_FORMAT_MOD_LINEAR`, the layout every importer takes.
const MOD_LINEAR: u64 = 0;

/// PCI vendor of NVIDIA, whose Linux driver attaches no fence to a dmabuf.
const NVIDIA: u32 = 0x10de;

/// `DMA_BUF_IOCTL_EXPORT_SYNC_FILE`, `_IOWR('b', 2, struct dma_buf_export_sync_file)`.
const EXPORT_SYNC_FILE: u64 = 0xC008_6202;

/// `DMA_BUF_SYNC_READ`, a fence that holds a reader back from writes.
const SYNC_READ: u32 = 1;

/// Linux names a GPU by its DRM render node, major and minor.
pub(super) type AdapterId = (u32, u32);

thread_local! {
    /// Device this thread's browsers share, alive while one of them is.
    static DEVICE: RefCell<Weak<Device>> = const { RefCell::new(Weak::new()) };
}

/// `struct dma_buf_export_sync_file`.
#[repr(C)]
struct ExportSyncFile {
    flags: u32,
    fd: i32,
}

/// Returns the fence of what `fd`'s writers have not finished, None for a
/// buffer or kernel without one.
fn write_fence(fd: BorrowedFd<'_>) -> Option<OwnedFd> {
    let mut export = ExportSyncFile {
        flags: SYNC_READ,
        fd: -1,
    };
    // SAFETY: the request takes this struct, which lives through the call
    let answered = unsafe { libc::ioctl(fd.as_raw_fd(), EXPORT_SYNC_FILE as _, &raw mut export) };
    // SAFETY: the kernel gave this descriptor to the caller alone
    (answered == 0 && export.fd >= 0).then(|| unsafe { OwnedFd::from_raw_fd(export.fd) })
}

/// Returns the inode of `fd`, which names a dmabuf for its life.
fn inode(fd: BorrowedFd<'_>) -> Result<u64, String> {
    // SAFETY: stat is plain data that fstat fills
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: fd is live and stat lives through the call
    if unsafe { libc::fstat(fd.as_raw_fd(), &raw mut stat) } != 0 {
        return Err("a dmabuf has no inode".to_owned());
    }
    Ok(stat.st_ino)
}

/// Dmabuf of a slot.
#[derive(Debug)]
pub(in crate::offscreen) struct Native {
    pub(super) fd: OwnedFd,
    pub(super) stride: u32,
    pub(super) offset: u64,
    pub(super) size: u64,
    pub(super) modifier: u64,
}

/// Returns the extensions a device needs.
fn needed_extensions() -> [&'static std::ffi::CStr; 6] {
    [
        ash::khr::external_memory_fd::NAME,
        ash::ext::external_memory_dma_buf::NAME,
        ash::ext::image_drm_format_modifier::NAME,
        ash::khr::external_semaphore_fd::NAME,
        ash::ext::queue_family_foreign::NAME,
        ash::khr::image_format_list::NAME,
    ]
}

/// Returns the Vulkan loader, loaded once for the process's life.
fn loader() -> Result<&'static ash::Entry, String> {
    static LOADER: OnceLock<Result<ash::Entry, String>> = OnceLock::new();
    LOADER
        .get_or_init(|| {
            // SAFETY: libvulkan is the system loader
            unsafe { ash::Entry::load() }.map_err(|e| format!("no Vulkan loader: {e}"))
        })
        .as_ref()
        .map_err(Clone::clone)
}

/// Returns a Vulkan instance.
fn instance() -> Result<ash::Instance, String> {
    let application = vk::ApplicationInfo::default().api_version(vk::API_VERSION_1_1);
    let info = vk::InstanceCreateInfo::default().application_info(&application);
    // SAFETY: info lives through the call
    unsafe { loader()?.create_instance(&info, None) }
        .map_err(|e| format!("no Vulkan instance: {e}"))
}

/// Returns the render node of `physical`, None without one.
fn render_node(instance: &ash::Instance, physical: vk::PhysicalDevice) -> Option<AdapterId> {
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
    (drm.has_render != 0).then_some((drm.render_major as u32, drm.render_minor as u32))
}

/// Returns the physical device `wanted` names, else the fastest kind there is.
fn physical_device(
    instance: &ash::Instance,
    wanted: Option<AdapterId>,
) -> Result<vk::PhysicalDevice, String> {
    // SAFETY: the instance is live
    let devices = unsafe { instance.enumerate_physical_devices() }
        .map_err(|e| format!("no Vulkan devices: {e}"))?;
    let rank = |physical: &vk::PhysicalDevice| {
        // SAFETY: physical comes from instance
        let properties = unsafe { instance.get_physical_device_properties(*physical) };
        match properties.device_type {
            vk::PhysicalDeviceType::DISCRETE_GPU => 3,
            vk::PhysicalDeviceType::INTEGRATED_GPU => 2,
            vk::PhysicalDeviceType::VIRTUAL_GPU => 1,
            _ => 0,
        }
    };
    let chosen = match wanted {
        Some(node) => devices
            .iter()
            .find(|physical| render_node(instance, **physical) == Some(node)),
        None => devices
            .iter()
            .filter(|physical| rank(physical) > 0)
            .max_by_key(|physical| rank(physical)),
    };
    chosen
        .copied()
        .ok_or_else(|| "no Vulkan device of that GPU".to_owned())
}

/// Returns the adapter of `choice`.
pub(super) fn adapter(choice: GpuChoice) -> Result<GpuAdapter, String> {
    if let GpuChoice::Adapter(adapter) = choice {
        return Ok(adapter);
    }
    let instance = instance()?;
    let node = physical_device(&instance, None)
        .ok()
        .and_then(|physical| render_node(&instance, physical));
    // SAFETY: nothing of the instance outlives this
    unsafe { instance.destroy_instance(None) };
    node.map(GpuAdapter)
        .ok_or_else(|| "no GPU with a render node".to_owned())
}

/// Returns the switch that pins Chromium to `adapter`.
pub(super) fn pin_switch(adapter: GpuAdapter) -> Option<(&'static str, String)> {
    let (_, minor) = adapter.0;
    Some(("render-node-override", format!("/dev/dri/renderD{minor}")))
}

/// Returns the Vulkan format of `format`.
fn vk_format(format: TextureFormat) -> vk::Format {
    match format {
        TextureFormat::Rgba8 => vk::Format::R8G8B8A8_UNORM,
        TextureFormat::Bgra8 => vk::Format::B8G8R8A8_UNORM,
    }
}

/// Vulkan objects everything else of the device keeps alive.
struct Core {
    instance: ash::Instance,
    physical: vk::PhysicalDevice,
    device: ash::Device,
    memory_fd: ash::khr::external_memory_fd::Device,
    semaphore_fd: ash::khr::external_semaphore_fd::Device,
    modifiers: ash::ext::image_drm_format_modifier::Device,
    memory_types: vk::PhysicalDeviceMemoryProperties,
    pool: vk::CommandPool,
    commands: vk::CommandBuffer,
    fence: vk::Fence,
    /// Waited on for Chromium's write; holds each frame's fence for its submit
    written: vk::Semaphore,
}

impl Drop for Core {
    fn drop(&mut self) {
        // SAFETY: every object below came from this device; the idle wait ends
        // their use and the last holder of Core is going
        unsafe {
            let _ = self.device.device_wait_idle();
            self.device.destroy_semaphore(self.written, None);
            self.device.destroy_fence(self.fence, None);
            self.device.destroy_command_pool(self.pool, None);
            self.device.destroy_device(None);
            self.instance.destroy_instance(None);
        }
    }
}

impl Core {
    /// Returns the planes this GPU's copies take in `format` under `modifier`,
    /// None for a modifier they do not take. An implicit layout has no entry.
    fn modifier_planes(&self, format: vk::Format, modifier: u64) -> Option<usize> {
        let mut list = vk::DrmFormatModifierPropertiesListEXT::default();
        let mut properties = vk::FormatProperties2::default().push_next(&mut list);
        // SAFETY: physical comes from instance; properties lives through the call
        unsafe {
            self.instance.get_physical_device_format_properties2(
                self.physical,
                format,
                &mut properties,
            );
        }
        let mut modifiers = vec![
            vk::DrmFormatModifierPropertiesEXT::default();
            list.drm_format_modifier_count as usize
        ];
        let mut list = vk::DrmFormatModifierPropertiesListEXT::default()
            .drm_format_modifier_properties(&mut modifiers);
        let mut properties = vk::FormatProperties2::default().push_next(&mut list);
        // SAFETY: as above; the array holds the count just read
        unsafe {
            self.instance.get_physical_device_format_properties2(
                self.physical,
                format,
                &mut properties,
            );
        }
        modifiers
            .iter()
            .find(|found| {
                found.drm_format_modifier == modifier
                    && found
                        .drm_format_modifier_tiling_features
                        .contains(vk::FormatFeatureFlags::TRANSFER_SRC)
            })
            .map(|found| found.drm_format_modifier_plane_count as usize)
    }

    /// Returns a memory type of `bits` with `flags`, else any of `bits`.
    fn memory_type(&self, bits: u32, flags: vk::MemoryPropertyFlags) -> Option<u32> {
        let types = &self.memory_types;
        let with_flags = (0..types.memory_type_count).find(|&index| {
            bits & (1 << index) != 0
                && types.memory_types[index as usize]
                    .property_flags
                    .contains(flags)
        });
        with_flags.or_else(|| (0..types.memory_type_count).find(|&i| bits & (1 << i) != 0))
    }
}

/// Who holds an image between copies.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Holder {
    /// This device, for a popup layer.
    Device,
    /// Nobody, for a slot never written.
    Nobody,
    /// The outside, in the layout it reads.
    Outside,
}

/// Image on the device with its memory.
pub(in crate::offscreen) struct Image {
    core: Rc<Core>,
    image: vk::Image,
    memory: vk::DeviceMemory,
    holder: Cell<Holder>,
    /// Fence of a frame's write, taken by the copy that reads it
    fence: Cell<Option<OwnedFd>>,
}

impl Drop for Image {
    fn drop(&mut self) {
        // SAFETY: every copy waits before it returns, which leaves the image
        // unused; both objects came from this device
        unsafe {
            self.core.device.destroy_image(self.image, None);
            self.core.device.free_memory(self.memory, None);
        }
    }
}

/// Staging memory of a browser's software frames.
pub(in crate::offscreen) struct Staging {
    core: Rc<Core>,
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    mapped: *mut u8,
    stride: usize,
}

impl Drop for Staging {
    fn drop(&mut self) {
        // SAFETY: as Image's
        unsafe {
            self.core.device.destroy_buffer(self.buffer, None);
            self.core.device.free_memory(self.memory, None);
        }
    }
}

/// CEF's dmabuf, kept open between frames.
pub(in crate::offscreen) struct Kept {
    /// Open while kept, which keeps its inode from naming another
    fd: OwnedFd,
    image: Rc<Image>,
}

/// Private device of this process's browsers. UI thread alone.
pub(in crate::offscreen) struct Device {
    adapter: GpuAdapter,
    core: Rc<Core>,
    queue: vk::Queue,
    family: u32,
    /// A dmabuf's writes are fenced and the kernel gives the fences
    fences_dmabufs: bool,
    lost: Cell<bool>,
}

/// Part of an image a copy names.
fn layers() -> vk::ImageSubresourceLayers {
    vk::ImageSubresourceLayers::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .layer_count(1)
}

/// Returns a copy of `width` by `height` from (`x`, `y`) to (`to_x`, `to_y`).
fn region_copy(x: u32, y: u32, to_x: u32, to_y: u32, width: u32, height: u32) -> vk::ImageCopy {
    vk::ImageCopy::default()
        .src_subresource(layers())
        .dst_subresource(layers())
        .src_offset(vk::Offset3D {
            x: x as i32,
            y: y as i32,
            z: 0,
        })
        .dst_offset(vk::Offset3D {
            x: to_x as i32,
            y: to_y as i32,
            z: 0,
        })
        .extent(vk::Extent3D {
            width,
            height,
            depth: 1,
        })
}

impl Device {
    /// Returns the device on `adapter` this thread's browsers share.
    pub(super) fn shared(adapter: GpuAdapter) -> Result<Rc<Self>, String> {
        DEVICE.with_borrow_mut(|device| {
            if let Some(device) = device.upgrade().filter(|device| !device.lost.get()) {
                return Ok(device);
            }
            let made = Rc::new(Self::open_on(adapter)?);
            *device = Rc::downgrade(&made);
            Ok(made)
        })
    }

    /// Opens a device on `adapter`, on a queue of the copy engine where the GPU
    /// has one apart from its 3D work.
    fn open_on(adapter: GpuAdapter) -> Result<Self, String> {
        let instance = instance()?;
        let physical = physical_device(&instance, Some(adapter.0))?;
        // SAFETY: physical comes from instance
        let families = unsafe { instance.get_physical_device_queue_family_properties(physical) };
        let transfer_only = families.iter().position(|family| {
            family.queue_flags.contains(vk::QueueFlags::TRANSFER)
                && !family
                    .queue_flags
                    .intersects(vk::QueueFlags::GRAPHICS | vk::QueueFlags::COMPUTE)
        });
        let any = families.iter().position(|family| {
            family
                .queue_flags
                .intersects(vk::QueueFlags::TRANSFER | vk::QueueFlags::GRAPHICS)
        });
        let family = transfer_only.or(any).ok_or("no queue that copies")? as u32;

        let priorities = [1.0];
        let queues = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(family)
            .queue_priorities(&priorities)];
        let extensions = needed_extensions().map(std::ffi::CStr::as_ptr);
        let info = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queues)
            .enabled_extension_names(&extensions);
        // SAFETY: physical comes from instance and info lives through the call
        let device = unsafe { instance.create_device(physical, &info, None) }
            .map_err(|e| format!("no Vulkan device: {e}"))?;
        // SAFETY: the family was asked for with one queue
        let queue = unsafe { device.get_device_queue(family, 0) };
        // SAFETY: each info lives through its call
        let (pool, commands, fence, written) = unsafe {
            let pool = device
                .create_command_pool(
                    &vk::CommandPoolCreateInfo::default()
                        .queue_family_index(family)
                        .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                    None,
                )
                .map_err(|e| format!("no command pool: {e}"))?;
            let commands = device
                .allocate_command_buffers(
                    &vk::CommandBufferAllocateInfo::default()
                        .command_pool(pool)
                        .command_buffer_count(1),
                )
                .map_err(|e| format!("no command buffer: {e}"))?[0];
            let fence = device
                .create_fence(&vk::FenceCreateInfo::default(), None)
                .map_err(|e| format!("no fence: {e}"))?;
            let written = device
                .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
                .map_err(|e| format!("no semaphore: {e}"))?;
            (pool, commands, fence, written)
        };
        let memory_fd = ash::khr::external_memory_fd::Device::new(&instance, &device);
        let semaphore_fd = ash::khr::external_semaphore_fd::Device::new(&instance, &device);
        let modifiers = ash::ext::image_drm_format_modifier::Device::new(&instance, &device);
        // SAFETY: physical comes from instance
        let memory_types = unsafe { instance.get_physical_device_memory_properties(physical) };
        // SAFETY: physical comes from instance
        let properties = unsafe { instance.get_physical_device_properties(physical) };
        let mut opened = Self {
            adapter,
            core: Rc::new(Core {
                instance,
                physical,
                device,
                memory_fd,
                semaphore_fd,
                modifiers,
                memory_types,
                pool,
                commands,
                fence,
                written,
            }),
            queue,
            family,
            fences_dmabufs: properties.vendor_id != NVIDIA,
            lost: Cell::new(false),
        };
        opened.fences_dmabufs &= opened.exports_fences();
        Ok(opened)
    }

    /// Returns whether the kernel gives a dmabuf's fences, which it does from
    /// Linux 5.20 on.
    fn exports_fences(&self) -> bool {
        let shape = Shape {
            width: 1,
            height: 1,
            format: TextureFormat::Bgra8,
        };
        self.make_exported(shape)
            .is_ok_and(|(native, _)| write_fence(native.fd.as_fd()).is_some())
    }

    /// Records a barrier of `image`. `acquire` takes it from the outside for
    /// `layout`; else gives it back from `layout`.
    fn share(&self, image: &Image, layout: vk::ImageLayout, acquire: bool) {
        let (ignored, foreign) = (vk::QUEUE_FAMILY_IGNORED, vk::QUEUE_FAMILY_FOREIGN_EXT);
        let general = vk::ImageLayout::GENERAL;
        let (from, to, source, target) = match (image.holder.get(), acquire) {
            (Holder::Device, _) => return,
            (Holder::Nobody, true) => (vk::ImageLayout::UNDEFINED, layout, ignored, ignored),
            (Holder::Outside, true) => (general, layout, foreign, self.family),
            (_, false) => (layout, general, self.family, foreign),
        };
        self.barrier(image.image, from, to, source, target);
        if !acquire {
            image.holder.set(Holder::Outside);
        }
    }

    fn barrier(
        &self,
        image: vk::Image,
        from: vk::ImageLayout,
        to: vk::ImageLayout,
        source: u32,
        target: u32,
    ) {
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
            .image(image)
            .subresource_range(range);
        // SAFETY: the command buffer is recording and image is of this device
        unsafe {
            self.core.device.cmd_pipeline_barrier(
                self.core.commands,
                vk::PipelineStageFlags::ALL_COMMANDS,
                vk::PipelineStageFlags::ALL_COMMANDS,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[barrier],
            );
        }
    }

    /// Records a copy between `from` and `to`.
    fn copy_image(&self, from: &Image, to: &Image, region: vk::ImageCopy) {
        let source = vk::ImageLayout::TRANSFER_SRC_OPTIMAL;
        let target = vk::ImageLayout::TRANSFER_DST_OPTIMAL;
        self.share(from, source, true);
        self.share(to, target, true);
        // SAFETY: the command buffer is recording and the images are in the
        // layouts named
        unsafe {
            self.core.device.cmd_copy_image(
                self.core.commands,
                from.image,
                source,
                to.image,
                target,
                &[region],
            );
        }
        self.share(to, target, false);
        self.share(from, source, false);
    }

    /// Records `copy`, keeping a fence it must wait on.
    fn record(&self, copy: &Copy<'_, Self>, fences: &mut Vec<OwnedFd>) {
        match copy {
            Copy::Frame { from, to, region } => {
                fences.extend(from.fence.take());
                let (x, y) = (region.x, region.y);
                let copied = region_copy(x, y, x, y, region.width, region.height);
                self.copy_image(from, to, copied);
            }
            Copy::Layer {
                from,
                to,
                width,
                height,
            } => {
                fences.extend(from.fence.take());
                self.barrier(
                    to.image,
                    vk::ImageLayout::UNDEFINED,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    vk::QUEUE_FAMILY_IGNORED,
                    vk::QUEUE_FAMILY_IGNORED,
                );
                self.copy_image(from, to, region_copy(0, 0, 0, 0, *width, *height));
                self.barrier(
                    to.image,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    vk::QUEUE_FAMILY_IGNORED,
                    vk::QUEUE_FAMILY_IGNORED,
                );
            }
            Copy::Popup { from, to, place } => {
                let copied = region_copy(0, 0, place.x, place.y, place.width, place.height);
                self.copy_image(from, to, copied);
            }
            Copy::Staged { from, to, region } => {
                let layout = vk::ImageLayout::TRANSFER_DST_OPTIMAL;
                self.share(to, layout, true);
                let copied = vk::BufferImageCopy::default()
                    .buffer_offset((region.y as usize * from.stride + region.x as usize * 4) as u64)
                    .buffer_row_length((from.stride / 4) as u32)
                    .image_subresource(layers())
                    .image_offset(vk::Offset3D {
                        x: region.x as i32,
                        y: region.y as i32,
                        z: 0,
                    })
                    .image_extent(vk::Extent3D {
                        width: region.width,
                        height: region.height,
                        depth: 1,
                    });
                // SAFETY: the command buffer is recording and the slot is in
                // the layout named
                unsafe {
                    self.core.device.cmd_copy_buffer_to_image(
                        self.core.commands,
                        from.buffer,
                        to.image,
                        layout,
                        &[copied],
                    );
                }
                self.share(to, layout, false);
            }
        }
    }

    /// Makes `fence` the wait of the next submit, False when the driver takes
    /// none.
    fn wait_on(&self, fence: OwnedFd) -> bool {
        let raw: RawFd = fence.as_raw_fd();
        let info = vk::ImportSemaphoreFdInfoKHR::default()
            .semaphore(self.core.written)
            .flags(vk::SemaphoreImportFlags::TEMPORARY)
            .handle_type(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD)
            .fd(raw);
        // SAFETY: info lives through the call; the driver owns the descriptor
        // from a successful import on
        match unsafe { self.core.semaphore_fd.import_semaphore_fd(&info) } {
            Ok(()) => {
                std::mem::forget(fence);
                true
            }
            Err(e) => {
                debug!("a dmabuf's fence did not import: {e}");
                false
            }
        }
    }

    /// Waits for the fence, reading the device's state while it does not come.
    fn wait(&self) -> Result<(), String> {
        let core = &self.core;
        loop {
            // SAFETY: the fence is of this device
            match unsafe { core.device.wait_for_fences(&[core.fence], true, WAIT_SLICE) } {
                Ok(()) => break,
                Err(vk::Result::TIMEOUT) => {}
                Err(e) => {
                    self.lost.set(e == vk::Result::ERROR_DEVICE_LOST);
                    return Err(format!("the private device is lost: {e}"));
                }
            }
        }
        // SAFETY: the fence has signaled and nothing waits on it
        unsafe { core.device.reset_fences(&[core.fence]) }
            .map_err(|e| format!("no fence reset: {e}"))
    }

    /// Returns a new image with a dmabuf of `shape` to export.
    fn make_exported(&self, shape: Shape) -> Result<(Native, Image), String> {
        let core = &self.core;
        let modifiers = [MOD_LINEAR];
        let mut list =
            vk::ImageDrmFormatModifierListCreateInfoEXT::default().drm_format_modifiers(&modifiers);
        let mut external = vk::ExternalMemoryImageCreateInfo::default()
            .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
        let create = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(vk_format(shape.format))
            .extent(vk::Extent3D {
                width: shape.width,
                height: shape.height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
            .usage(
                vk::ImageUsageFlags::TRANSFER_DST
                    | vk::ImageUsageFlags::TRANSFER_SRC
                    | vk::ImageUsageFlags::SAMPLED,
            )
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .push_next(&mut list)
            .push_next(&mut external);
        // SAFETY: each info lives through its call, the image is fresh and the
        // memory is dedicated to it
        unsafe {
            let image = core
                .device
                .create_image(&create, None)
                .map_err(|e| format!("no slot of {}x{}: {e}", shape.width, shape.height))?;
            let needs = core.device.get_image_memory_requirements(image);
            let index = core
                .memory_type(
                    needs.memory_type_bits,
                    vk::MemoryPropertyFlags::DEVICE_LOCAL,
                )
                .ok_or("no memory type for a slot")?;
            let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(image);
            let mut export = vk::ExportMemoryAllocateInfo::default()
                .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
            let allocate = vk::MemoryAllocateInfo::default()
                .allocation_size(needs.size)
                .memory_type_index(index)
                .push_next(&mut dedicated)
                .push_next(&mut export);
            let memory = core
                .device
                .allocate_memory(&allocate, None)
                .map_err(|e| format!("no slot memory: {e}"))?;
            core.device
                .bind_image_memory(image, memory, 0)
                .map_err(|e| format!("no slot memory: {e}"))?;
            let fd = core
                .memory_fd
                .get_memory_fd(
                    &vk::MemoryGetFdInfoKHR::default()
                        .memory(memory)
                        .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT),
                )
                .map_err(|e| format!("no dmabuf of a slot: {e}"))?;
            let layout = core.device.get_image_subresource_layout(
                image,
                vk::ImageSubresource::default()
                    .aspect_mask(vk::ImageAspectFlags::MEMORY_PLANE_0_EXT),
            );
            let mut properties = vk::ImageDrmFormatModifierPropertiesEXT::default();
            core.modifiers
                .get_image_drm_format_modifier_properties(image, &mut properties)
                .map_err(|e| format!("no modifier of a slot: {e}"))?;
            let native = Native {
                fd: OwnedFd::from_raw_fd(fd),
                stride: layout.row_pitch as u32,
                offset: layout.offset,
                size: needs.size,
                modifier: properties.drm_format_modifier,
            };
            Ok((
                native,
                Image {
                    core: Rc::clone(core),
                    image,
                    memory,
                    holder: Cell::new(Holder::Nobody),
                    fence: Cell::new(None),
                },
            ))
        }
    }

    /// Imports the dmabuf of `frame`, its planes in one buffer.
    fn import(&self, frame: &SharedTextureFrame<'_>, shape: Shape) -> Result<Image, String> {
        let core = &self.core;
        let planes: Vec<_> = frame.planes().collect();
        let first = planes.first().ok_or("a frame came without its planes")?;
        let format = vk_format(shape.format);
        let modifier = frame.modifier();
        match core.modifier_planes(format, modifier) {
            Some(count) if count == planes.len() => {}
            Some(count) => {
                return Err(format!(
                    "CEF's dmabuf has {} planes where modifier {modifier:#x} has {count}",
                    planes.len()
                ));
            }
            None => {
                return Err(format!(
                    "this GPU's Vulkan cannot copy from a dmabuf of modifier {modifier:#x}"
                ));
            }
        }
        let layouts: Vec<vk::SubresourceLayout> = planes
            .iter()
            .map(|plane| vk::SubresourceLayout {
                offset: plane.offset,
                size: 0,
                row_pitch: u64::from(plane.stride),
                array_pitch: 0,
                depth_pitch: 0,
            })
            .collect();
        let mut explicit = vk::ImageDrmFormatModifierExplicitCreateInfoEXT::default()
            .drm_format_modifier(modifier)
            .plane_layouts(&layouts);
        let mut external = vk::ExternalMemoryImageCreateInfo::default()
            .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
        let create = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(format)
            .extent(vk::Extent3D {
                width: shape.width,
                height: shape.height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
            .usage(vk::ImageUsageFlags::TRANSFER_SRC)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .push_next(&mut explicit)
            .push_next(&mut external);
        // SAFETY: each info lives through its call and the image is fresh. The
        // driver owns the duplicate from a successful allocation on
        unsafe {
            let image = core
                .device
                .create_image(&create, None)
                .map_err(|e| format!("CEF's dmabuf does not import: {e}"))?;
            let needs = core.device.get_image_memory_requirements(image);
            let duplicate = first
                .fd
                .try_clone_to_owned()
                .map_err(|e| format!("no dmabuf descriptor: {e}"))?;
            let mut fd_bits = vk::MemoryFdPropertiesKHR::default();
            core.memory_fd
                .get_memory_fd_properties(
                    vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT,
                    duplicate.as_raw_fd(),
                    &mut fd_bits,
                )
                .map_err(|e| format!("CEF's dmabuf does not import: {e}"))?;
            let bits = needs.memory_type_bits & fd_bits.memory_type_bits;
            let index = core
                .memory_type(bits, vk::MemoryPropertyFlags::empty())
                .ok_or("no memory type for CEF's dmabuf")?;
            let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(image);
            let mut import = vk::ImportMemoryFdInfoKHR::default()
                .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT)
                .fd(duplicate.as_raw_fd());
            let allocate = vk::MemoryAllocateInfo::default()
                .allocation_size(needs.size)
                .memory_type_index(index)
                .push_next(&mut dedicated)
                .push_next(&mut import);
            let memory = core
                .device
                .allocate_memory(&allocate, None)
                .map_err(|e| format!("CEF's dmabuf does not import: {e}"))?;
            std::mem::forget(duplicate);
            core.device
                .bind_image_memory(image, memory, 0)
                .map_err(|e| format!("CEF's dmabuf does not bind: {e}"))?;
            Ok(Image {
                core: Rc::clone(core),
                image,
                memory,
                holder: Cell::new(Holder::Outside),
                fence: Cell::new(None),
            })
        }
    }
}

impl FrameDevice for Device {
    type Native = Native;
    type Slot = Image;
    type Texture = Rc<Image>;
    type Kept = Vec<Kept>;
    type Staging = Staging;

    fn make_slot(&self, shape: Shape) -> Result<(Native, Image), String> {
        self.make_exported(shape)
    }

    fn make_layer(&self, shape: Shape) -> Result<Rc<Image>, String> {
        let core = &self.core;
        let create = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(vk_format(shape.format))
            .extent(vk::Extent3D {
                width: shape.width,
                height: shape.height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::TRANSFER_SRC)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        // SAFETY: each info lives through its call and the image is fresh
        unsafe {
            let image = core
                .device
                .create_image(&create, None)
                .map_err(|e| format!("no popup layer: {e}"))?;
            let needs = core.device.get_image_memory_requirements(image);
            let index = core
                .memory_type(
                    needs.memory_type_bits,
                    vk::MemoryPropertyFlags::DEVICE_LOCAL,
                )
                .ok_or("no memory type for a layer")?;
            let memory = core
                .device
                .allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(needs.size)
                        .memory_type_index(index),
                    None,
                )
                .map_err(|e| format!("no popup layer: {e}"))?;
            core.device
                .bind_image_memory(image, memory, 0)
                .map_err(|e| format!("no popup layer: {e}"))?;
            Ok(Rc::new(Image {
                core: Rc::clone(core),
                image,
                memory,
                holder: Cell::new(Holder::Device),
                fence: Cell::new(None),
            }))
        }
    }

    fn make_staging(&self, shape: Shape) -> Result<Staging, String> {
        let core = &self.core;
        let stride = shape.width as usize * 4;
        let size = (stride * shape.height as usize) as u64;
        // SAFETY: each info lives through its call; the buffer is fresh and maps
        // whole for its life
        unsafe {
            let buffer = core
                .device
                .create_buffer(
                    &vk::BufferCreateInfo::default()
                        .size(size)
                        .usage(vk::BufferUsageFlags::TRANSFER_SRC),
                    None,
                )
                .map_err(|e| format!("no staging memory: {e}"))?;
            let needs = core.device.get_buffer_memory_requirements(buffer);
            let flags =
                vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT;
            let index = core
                .memory_type(needs.memory_type_bits, flags)
                .ok_or("no memory type for staging")?;
            let memory = core
                .device
                .allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(needs.size)
                        .memory_type_index(index),
                    None,
                )
                .map_err(|e| format!("no staging memory: {e}"))?;
            core.device
                .bind_buffer_memory(buffer, memory, 0)
                .map_err(|e| format!("no staging memory: {e}"))?;
            let mapped = core
                .device
                .map_memory(memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())
                .map_err(|e| format!("no staging map: {e}"))?;
            Ok(Staging {
                core: Rc::clone(core),
                buffer,
                memory,
                mapped: mapped.cast(),
                stride,
            })
        }
    }

    fn stage(&self, staging: &mut Staging, frame: &PixelFrame<'_>, region: PixelRect) {
        let row_bytes = region.width as usize * 4;
        for row in region.y as usize..(region.y + region.height) as usize {
            let start = row * frame.stride() + region.x as usize * 4;
            let from = &frame.bytes()[start..start + row_bytes];
            // SAFETY: the mapping holds stride bytes for each row of the shape,
            // whose region lies inside it; no copy runs while this writes
            unsafe {
                let to = staging
                    .mapped
                    .add(row * staging.stride + region.x as usize * 4);
                std::ptr::copy_nonoverlapping(from.as_ptr(), to, row_bytes);
            }
        }
    }

    fn run(&self, copies: &[Copy<'_, Self>]) -> Result<(), String> {
        if copies.is_empty() {
            return Ok(());
        }
        let core = &self.core;
        let mut fences = Vec::new();
        // SAFETY: the wait below ends the buffer's last use, which leaves it
        // free at each reset
        unsafe {
            core.device
                .reset_command_buffer(core.commands, vk::CommandBufferResetFlags::empty())
                .map_err(|e| format!("no command reset: {e}"))?;
            core.device
                .begin_command_buffer(core.commands, &vk::CommandBufferBeginInfo::default())
                .map_err(|e| format!("no command begin: {e}"))?;
        }
        for copy in copies {
            self.record(copy, &mut fences);
        }
        // SAFETY: the buffer is recording
        unsafe { core.device.end_command_buffer(core.commands) }
            .map_err(|e| format!("no command end: {e}"))?;

        // Chromium writes one frame at a time, which leaves one fence to wait on
        let waits = fences.pop().is_some_and(|fence| self.wait_on(fence));
        let semaphores = [core.written];
        let stages = [vk::PipelineStageFlags::ALL_COMMANDS];
        let buffers = [core.commands];
        let mut submit = vk::SubmitInfo::default().command_buffers(&buffers);
        if waits {
            submit = submit
                .wait_semaphores(&semaphores)
                .wait_dst_stage_mask(&stages);
        }
        // SAFETY: every array lives through the call and the fence is unsignaled
        if let Err(e) = unsafe { core.device.queue_submit(self.queue, &[submit], core.fence) } {
            self.lost.set(e == vk::Result::ERROR_DEVICE_LOST);
            return Err(format!("no submit: {e}"));
        }
        self.wait()
    }

    fn is_lost(&self) -> bool {
        self.lost.get()
    }

    fn waits_on_textures(&self) -> bool {
        self.fences_dmabufs
    }

    fn reopen(&self) -> Result<Rc<Self>, String> {
        Self::shared(self.adapter)
    }
}

impl OpensFrames for Device {
    fn open(
        &self,
        kept: &mut Vec<Kept>,
        frame: &SharedTextureFrame<'_>,
        shape: Shape,
    ) -> Result<Rc<Image>, String> {
        let first = frame
            .planes()
            .next()
            .ok_or("a frame came without its planes")?;
        let fence = write_fence(first.fd.as_fd());
        let number = inode(first.fd.as_fd())?;
        let open = |kept: &&Kept| inode(kept.fd.as_fd()).is_ok_and(|kept| kept == number);
        if let Some(found) = kept.iter().find(open) {
            found.image.fence.set(fence);
            return Ok(Rc::clone(&found.image));
        }
        let image = Rc::new(self.import(frame, shape)?);
        image.fence.set(fence);
        let fd = first
            .fd
            .try_clone_to_owned()
            .map_err(|e| format!("no dmabuf descriptor: {e}"))?;
        if kept.len() == KEPT_OPEN {
            kept.remove(0);
        }
        kept.push(Kept {
            fd,
            image: Rc::clone(&image),
        });
        Ok(image)
    }
}

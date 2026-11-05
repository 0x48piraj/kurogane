//! macOS private device of the frame ring.
//!
//! A Metal device and command queue of Kurogane's own blit each frame into
//! IOSurfaces Kurogane makes. Everything but the surfaces stays on CEF's UI
//! thread, the one thread that paints.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::ptr::NonNull;
use std::rc::{Rc, Weak};

use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::ProtocolObject;
use objc2_core_foundation::{CFDictionary, CFNumber, CFRetained, CFString};
use objc2_io_surface::{
    IOSurfaceRef, kIOSurfaceBytesPerElement, kIOSurfaceHeight, kIOSurfacePixelFormat,
    kIOSurfaceWidth,
};
use objc2_metal::{
    MTLBlitCommandEncoder, MTLBuffer, MTLCommandBuffer, MTLCommandBufferError,
    MTLCommandBufferStatus, MTLCommandEncoder, MTLCommandQueue, MTLCopyAllDevices,
    MTLCreateSystemDefaultDevice, MTLDevice, MTLOrigin, MTLPixelFormat, MTLResourceOptions,
    MTLSize, MTLStorageMode, MTLTexture, MTLTextureDescriptor, MTLTextureUsage,
};

use super::writer::{Copy, FrameDevice, OpensFrames};
use super::{GpuAdapter, GpuChoice, Shape};
use crate::offscreen::frame::{PixelFrame, PixelRect, SharedTextureFrame, TextureFormat};

/// Textures of CEF's a browser keeps open, more than its pool hands out at a
/// steady size.
const KEPT_OPEN: usize = 4;

/// macOS names a GPU by its registry id.
pub(super) type AdapterId = u64;

thread_local! {
    /// Device this thread's browsers share, alive while one of them is.
    static DEVICE: RefCell<Weak<Device>> = const { RefCell::new(Weak::new()) };
}

/// IOSurface of a slot.
#[derive(Debug)]
pub(in crate::offscreen) struct Native(CFRetained<IOSurfaceRef>);

// SAFETY: IOSurface objects are thread safe (IOSurfaceRef.h); hosts only
// read this one's identity from other threads
unsafe impl Send for Native {}
// SAFETY: as Send
unsafe impl Sync for Native {}

impl Native {
    /// The surface.
    pub(super) fn surface(&self) -> &IOSurfaceRef {
        &self.0
    }
}

/// Returns the adapter of `choice`.
pub(super) fn adapter(choice: GpuChoice) -> Result<GpuAdapter, String> {
    match choice {
        GpuChoice::Adapter(adapter) => Ok(adapter),
        GpuChoice::HighPerformance => {
            let device = MTLCreateSystemDefaultDevice().ok_or("no Metal device")?;
            Ok(GpuAdapter(device.registryID()))
        }
    }
}

/// Returns the switch that pins Chromium to `adapter`. Chromium picks its own
/// GPU on macOS, the system's default.
pub(super) fn pin_switch(_adapter: GpuAdapter) -> Option<(&'static str, String)> {
    None
}

/// Returns the Metal format of `format`.
fn pixel_format(format: TextureFormat) -> MTLPixelFormat {
    match format {
        TextureFormat::Rgba8 => MTLPixelFormat::RGBA8Unorm,
        TextureFormat::Bgra8 => MTLPixelFormat::BGRA8Unorm,
    }
}

/// Returns the IOSurface pixel format of `format`, its four characters.
fn surface_format(format: TextureFormat) -> i32 {
    match format {
        TextureFormat::Rgba8 => i32::from_be_bytes(*b"RGBA"),
        TextureFormat::Bgra8 => i32::from_be_bytes(*b"BGRA"),
    }
}

/// Returns a new IOSurface of `shape`.
fn make_surface(shape: Shape) -> Result<CFRetained<IOSurfaceRef>, String> {
    let number = |value: i32| CFNumber::new_i32(value);
    let width = number(shape.width as i32);
    let height = number(shape.height as i32);
    let bytes = number(4);
    let format = number(surface_format(shape.format));
    // SAFETY: the keys are immutable CFStrings the framework exports
    let keys: [&CFString; 4] = unsafe {
        [
            kIOSurfaceWidth,
            kIOSurfaceHeight,
            kIOSurfaceBytesPerElement,
            kIOSurfacePixelFormat,
        ]
    };
    let values: [&CFNumber; 4] = [&width, &height, &bytes, &format];
    let properties = CFDictionary::from_slices(&keys, &values);
    // SAFETY: the dictionary holds the keys IOSurfaceCreate documents
    unsafe { IOSurfaceRef::new(properties.as_opaque()) }
        .ok_or_else(|| format!("no IOSurface of {}x{}", shape.width, shape.height))
}

type Texture = Retained<ProtocolObject<dyn MTLTexture>>;

/// Staging memory of a browser's software frames.
pub(in crate::offscreen) struct Staging {
    buffer: Retained<ProtocolObject<dyn MTLBuffer>>,
    stride: usize,
}

/// CEF's surface, kept open between frames.
pub(in crate::offscreen) struct Kept {
    /// Retained while kept, which keeps its id from naming another
    surface: CFRetained<IOSurfaceRef>,
    texture: Texture,
}

/// Private device of this process's browsers. UI thread alone.
pub(in crate::offscreen) struct Device {
    adapter: GpuAdapter,
    device: Retained<ProtocolObject<dyn MTLDevice>>,
    queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
    lost: Cell<bool>,
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

    /// Opens a device and queue on `adapter`.
    fn open_on(adapter: GpuAdapter) -> Result<Self, String> {
        let device = MTLCopyAllDevices()
            .iter()
            .find(|device| device.registryID() == adapter.0)
            .ok_or("no Metal device of that registry id")?;
        let queue = device.newCommandQueue().ok_or("no command queue")?;
        Ok(Self {
            adapter,
            device,
            queue,
            lost: Cell::new(false),
        })
    }

    /// Returns a texture over `surface`, for a shape of `shape`.
    fn wrap(&self, surface: &IOSurfaceRef, shape: Shape) -> Result<Texture, String> {
        // SAFETY: the sizes are the surface's
        let descriptor = unsafe {
            MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
                pixel_format(shape.format),
                shape.width as usize,
                shape.height as usize,
                false,
            )
        };
        descriptor.setUsage(MTLTextureUsage::ShaderRead | MTLTextureUsage::ShaderWrite);
        descriptor.setStorageMode(if self.device.hasUnifiedMemory() {
            MTLStorageMode::Shared
        } else {
            MTLStorageMode::Managed
        });
        self.device
            .newTextureWithDescriptor_iosurface_plane(&descriptor, surface, 0)
            .ok_or_else(|| "an IOSurface does not wrap on Kurogane's device".to_owned())
    }
}

/// Returns Metal's size of `width` by `height`.
fn size(width: u32, height: u32) -> MTLSize {
    MTLSize {
        width: width as usize,
        height: height as usize,
        depth: 1,
    }
}

/// Returns Metal's origin of (`x`, `y`).
fn at(x: u32, y: u32) -> MTLOrigin {
    MTLOrigin {
        x: x as usize,
        y: y as usize,
        z: 0,
    }
}

impl FrameDevice for Device {
    type Native = Native;
    type Slot = Texture;
    type Texture = Texture;
    type Kept = Vec<Kept>;
    type Staging = Staging;

    fn make_slot(&self, shape: Shape) -> Result<(Native, Texture), String> {
        let surface = make_surface(shape)?;
        let texture = self.wrap(&surface, shape)?;
        Ok((Native(surface), texture))
    }

    fn make_layer(&self, shape: Shape) -> Result<Texture, String> {
        // SAFETY: the sizes are the popup's
        let descriptor = unsafe {
            MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
                pixel_format(shape.format),
                shape.width as usize,
                shape.height as usize,
                false,
            )
        };
        descriptor.setUsage(MTLTextureUsage::ShaderRead);
        descriptor.setStorageMode(MTLStorageMode::Private);
        self.device
            .newTextureWithDescriptor(&descriptor)
            .ok_or_else(|| "no popup layer".to_owned())
    }

    fn make_staging(&self, shape: Shape) -> Result<Staging, String> {
        let stride = shape.width as usize * 4;
        let buffer = self
            .device
            .newBufferWithLength_options(
                stride * shape.height as usize,
                MTLResourceOptions::StorageModeShared,
            )
            .ok_or("no staging memory")?;
        Ok(Staging { buffer, stride })
    }

    fn stage(&self, staging: &mut Staging, frame: &PixelFrame<'_>, region: PixelRect) {
        let contents: NonNull<c_void> = staging.buffer.contents();
        let row_bytes = region.width as usize * 4;
        for row in region.y as usize..(region.y + region.height) as usize {
            let start = row * frame.stride() + region.x as usize * 4;
            let from = &frame.bytes()[start..start + row_bytes];
            // SAFETY: the buffer holds stride bytes for each row of the shape,
            // whose region lies inside it; no copy runs while this writes
            unsafe {
                let to = contents
                    .as_ptr()
                    .cast::<u8>()
                    .add(row * staging.stride + region.x as usize * 4);
                std::ptr::copy_nonoverlapping(from.as_ptr(), to, row_bytes);
            }
        }
    }

    fn run(&self, copies: &[Copy<'_, Self>]) -> Result<(), String> {
        if copies.is_empty() {
            return Ok(());
        }
        autoreleasepool(|_| {
            let commands = self.queue.commandBuffer().ok_or("no command buffer")?;
            let blit = commands.blitCommandEncoder().ok_or("no blit encoder")?;
            for copy in copies {
                // SAFETY: the textures and buffers live until the wait below
                // ends and every region lies inside them
                unsafe {
                    match copy {
                        Copy::Frame { from, to, region } => blit
                            .copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toTexture_destinationSlice_destinationLevel_destinationOrigin(
                                from,
                                0,
                                0,
                                at(region.x, region.y),
                                size(region.width, region.height),
                                to,
                                0,
                                0,
                                at(region.x, region.y),
                            ),
                        Copy::Layer {
                            from,
                            to,
                            width,
                            height,
                        } => blit
                            .copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toTexture_destinationSlice_destinationLevel_destinationOrigin(
                                from,
                                0,
                                0,
                                at(0, 0),
                                size(*width, *height),
                                to,
                                0,
                                0,
                                at(0, 0),
                            ),
                        Copy::Popup { from, to, place } => blit
                            .copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toTexture_destinationSlice_destinationLevel_destinationOrigin(
                                from,
                                0,
                                0,
                                at(0, 0),
                                size(place.width, place.height),
                                to,
                                0,
                                0,
                                at(place.x, place.y),
                            ),
                        Copy::Staged { from, to, region } => blit
                            .copyFromBuffer_sourceOffset_sourceBytesPerRow_sourceBytesPerImage_sourceSize_toTexture_destinationSlice_destinationLevel_destinationOrigin(
                                &from.buffer,
                                region.y as usize * from.stride + region.x as usize * 4,
                                from.stride,
                                0,
                                size(region.width, region.height),
                                to,
                                0,
                                0,
                                at(region.x, region.y),
                            ),
                    }
                }
            }
            blit.endEncoding();
            commands.commit();
            commands.waitUntilCompleted();
            if commands.status() == MTLCommandBufferStatus::Error {
                let error = commands.error();
                if error.as_ref().is_some_and(|error| {
                    error.code() == MTLCommandBufferError::DeviceRemoved.0 as isize
                }) {
                    self.lost.set(true);
                }
                let reason = error.map_or_else(
                    || "no reason given".to_owned(),
                    |error| error.localizedDescription().to_string(),
                );
                return Err(format!("a blit failed: {reason}"));
            }
            Ok(())
        })
    }

    fn is_lost(&self) -> bool {
        self.lost.get()
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
    ) -> Result<Texture, String> {
        let surface = NonNull::new(frame.io_surface().as_ptr().cast::<IOSurfaceRef>())
            .ok_or("a frame came without its surface")?;
        // SAFETY: CEF lends a live surface for the callback
        let surface = unsafe { surface.as_ref() };
        let id = surface.id();
        if let Some(found) = kept.iter().find(|kept| kept.surface.id() == id) {
            return Ok(found.texture.clone());
        }
        if surface.width() != shape.width as usize || surface.height() != shape.height as usize {
            return Err(format!(
                "CEF's surface is {}x{}, its frame {}x{}",
                surface.width(),
                surface.height(),
                shape.width,
                shape.height
            ));
        }
        let texture = self.wrap(surface, shape)?;
        if kept.len() == KEPT_OPEN {
            kept.remove(0);
        }
        kept.push(Kept {
            // SAFETY: the pointer is a live CF object; the retain balances the drop
            surface: unsafe { CFRetained::retain(NonNull::from(surface)) },
            texture: texture.clone(),
        });
        Ok(texture)
    }
}

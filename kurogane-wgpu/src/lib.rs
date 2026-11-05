//! Offscreen Kurogane browsers as wgpu textures.
//!
//! [`Frames`] imports each slot of a browser's frame ring into the host's
//! device once and turns its leases into textures. Kurogane copies each frame
//! on its own device and queue. Nothing here touches the host's queue but a
//! barrier the backend asks for.
//!
//! The host makes its device on the GPU the browser runs on, which [`adapter`]
//! finds, with the features of [`required_features`].
//!
//! ```no_run
//! # async fn example(
//! #     instance: wgpu::Instance,
//! #     app: kurogane::AppInstance,
//! #     browser: kurogane::OffscreenBrowser,
//! # ) -> Result<(), Box<dyn std::error::Error>> {
//! let gpu = app.gpu_adapter().ok_or("an application without gpu_frames")?;
//! let adapter = kurogane_wgpu::adapter(&instance, wgpu::Backends::PRIMARY, gpu)
//!     .await
//!     .ok_or("no adapter on the browser's GPU")?;
//! let (device, queue) = adapter
//!     .request_device(&wgpu::DeviceDescriptor {
//!         required_features: kurogane_wgpu::required_features(&adapter),
//!         ..Default::default()
//!     })
//!     .await?;
//! let mut frames = kurogane_wgpu::Frames::new(&device, &queue, browser.gpu_frames())?;
//!
//! // Each redraw takes the newest frame, which returns the one shown before
//! let mut shown = None;
//! if let Some(frame) = frames.latest()? {
//!     shown = Some(frame);
//! }
//! if let Some(frame) = &shown {
//!     // Bind frame.view() and submit the draw
//! }
//! # Ok(())
//! # }
//! ```

#[cfg(windows)]
mod dx12;
#[cfg(target_os = "macos")]
mod metal;
#[cfg(not(target_os = "macos"))]
mod vulkan;

use std::collections::HashMap;
use std::fmt;

use kurogane::{GpuAdapter, GpuFrame, GpuFrames, PixelRect, SlotId, TextureFormat};

/// Blend state a frame's premultiplied alpha takes.
pub const BLEND: wgpu::BlendState = wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING;

/// Why a ring cannot reach a device.
#[derive(Debug)]
#[non_exhaustive]
pub enum ImportError {
    /// The device runs on a backend the ring has no import for.
    UnsupportedBackend,
    /// The device runs on another GPU than the ring.
    WrongAdapter,
    /// The device was made without these features.
    MissingFeatures(wgpu::Features),
    /// A slot did not import, for the reason named.
    Slot(String),
}

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedBackend => write!(f, "the device's backend has no ring import"),
            Self::WrongAdapter => write!(f, "the device runs on another GPU than the ring"),
            Self::MissingFeatures(features) => {
                write!(f, "the device lacks features {features:?}")
            }
            Self::Slot(problem) => write!(f, "a slot did not import: {problem}"),
        }
    }
}

impl std::error::Error for ImportError {}

/// Returns the features a device on `adapter` needs to import a ring.
pub fn required_features(adapter: &wgpu::Adapter) -> wgpu::Features {
    match adapter.get_info().backend {
        #[cfg(not(target_os = "macos"))]
        wgpu::Backend::Vulkan => vulkan::NEEDED,
        _ => wgpu::Features::empty(),
    }
}

/// Returns the adapter of `instance` on the GPU `gpu`, None when it shows none.
///
/// An instance leaves out a driver that claims no conformance unless its flags
/// hold [`wgpu::InstanceFlags::ALLOW_UNDERLYING_NONCOMPLIANT_ADAPTER`].
pub async fn adapter(
    instance: &wgpu::Instance,
    backends: wgpu::Backends,
    gpu: GpuAdapter,
) -> Option<wgpu::Adapter> {
    instance
        .enumerate_adapters(backends)
        .await
        .into_iter()
        .find(|adapter| gpu_of(adapter, gpu))
}

/// Returns whether `adapter` is the GPU `gpu`.
fn gpu_of(adapter: &wgpu::Adapter, gpu: GpuAdapter) -> bool {
    match adapter.get_info().backend {
        #[cfg(windows)]
        wgpu::Backend::Dx12 => dx12::adapter_gpu(adapter) == Some(gpu),
        #[cfg(not(target_os = "macos"))]
        wgpu::Backend::Vulkan => vulkan::adapter_gpu(adapter) == Some(gpu),
        #[cfg(target_os = "macos")]
        wgpu::Backend::Metal => metal::is_gpu(adapter, gpu),
        _ => false,
    }
}

/// Backend the device runs on.
#[derive(Clone, Copy)]
enum Backend {
    #[cfg(windows)]
    Dx12,
    #[cfg(not(target_os = "macos"))]
    Vulkan,
    #[cfg(target_os = "macos")]
    Metal,
}

impl Backend {
    /// Returns the features a device of this backend needs.
    fn needed(self) -> wgpu::Features {
        match self {
            #[cfg(windows)]
            Self::Dx12 => wgpu::Features::empty(),
            #[cfg(not(target_os = "macos"))]
            Self::Vulkan => vulkan::NEEDED,
            #[cfg(target_os = "macos")]
            Self::Metal => wgpu::Features::empty(),
        }
    }

    /// Imports the slot of `lease` as a texture of `desc`.
    fn import(
        self,
        device: &wgpu::Device,
        lease: &GpuFrame,
        desc: &wgpu::TextureDescriptor<'_>,
    ) -> Result<wgpu::Texture, ImportError> {
        match self {
            #[cfg(windows)]
            Self::Dx12 => dx12::import(device, lease, desc),
            #[cfg(not(target_os = "macos"))]
            Self::Vulkan => vulkan::import(device, lease, desc),
            #[cfg(target_os = "macos")]
            Self::Metal => metal::import(device, lease, desc),
        }
    }

    /// Takes `texture` from the ring's device for `queue`.
    fn acquire(self, device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture) {
        match self {
            #[cfg(windows)]
            Self::Dx12 => dx12::acquire(device, queue, texture),
            #[cfg(not(target_os = "macos"))]
            Self::Vulkan => vulkan::acquire(device, queue, texture),
            #[cfg(target_os = "macos")]
            Self::Metal => metal::acquire(device, queue, texture),
        }
    }

    /// Gives `texture` back to the ring's device.
    fn release(self, device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture) {
        match self {
            #[cfg(windows)]
            Self::Dx12 => dx12::release(device, queue, texture),
            #[cfg(not(target_os = "macos"))]
            Self::Vulkan => vulkan::release(device, queue, texture),
            #[cfg(target_os = "macos")]
            Self::Metal => metal::release(device, queue, texture),
        }
    }
}

/// Returns the backend of `device` and its GPU.
fn backend_of(device: &wgpu::Device) -> Result<(Backend, Option<GpuAdapter>), ImportError> {
    #[cfg(windows)]
    if let Some(gpu) = dx12::device_gpu(device) {
        return Ok((Backend::Dx12, gpu));
    }
    #[cfg(not(target_os = "macos"))]
    if let Some(gpu) = vulkan::device_gpu(device) {
        return Ok((Backend::Vulkan, gpu));
    }
    #[cfg(target_os = "macos")]
    if let Some(gpu) = metal::device_gpu(device) {
        return Ok((Backend::Metal, gpu));
    }
    Err(ImportError::UnsupportedBackend)
}

/// Slot imported into the host's device.
#[derive(Clone)]
struct Slot {
    id: SlotId,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    srgb_view: wgpu::TextureView,
}

/// A browser's frames on the host's device.
pub struct Frames {
    device: wgpu::Device,
    queue: wgpu::Queue,
    ring: GpuFrames,
    backend: Backend,
    slots: HashMap<SlotId, Slot>,
    /// Size of the newest frame; a slot of another goes
    size: (u32, u32),
}

impl Frames {
    /// Frames of `ring` on `device` and the `queue` that reads them.
    ///
    /// # Errors
    ///
    /// [`ImportError::UnsupportedBackend`], [`ImportError::WrongAdapter`] or
    /// [`ImportError::MissingFeatures`].
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        ring: GpuFrames,
    ) -> Result<Self, ImportError> {
        let (backend, gpu) = backend_of(device)?;
        if gpu != Some(ring.adapter()) {
            return Err(ImportError::WrongAdapter);
        }
        let needed = backend.needed();
        if !device.features().contains(needed) {
            return Err(ImportError::MissingFeatures(needed));
        }
        Ok(Self {
            device: device.clone(),
            queue: queue.clone(),
            ring,
            backend,
            slots: HashMap::new(),
            size: (0, 0),
        })
    }

    /// Leases the newest frame not yet leased, None when no new one waits.
    ///
    /// A host keeps showing the [`Frame`] it holds until a newer one comes,
    /// then drops the old one.
    ///
    /// # Errors
    ///
    /// [`ImportError::Slot`] for a slot the device cannot import.
    pub fn latest(&mut self) -> Result<Option<Frame>, ImportError> {
        let Some(lease) = self.ring.latest() else {
            return Ok(None);
        };
        let size = (lease.width(), lease.height());
        if size != self.size {
            self.size = size;
            self.slots.clear();
        }
        let slot = match self.slots.get(&lease.slot()) {
            Some(slot) => slot.clone(),
            None => {
                let slot = self.import(&lease)?;
                self.slots.insert(lease.slot(), slot.clone());
                slot
            }
        };
        self.backend
            .acquire(&self.device, &self.queue, &slot.texture);
        Ok(Some(Frame {
            device: self.device.clone(),
            queue: self.queue.clone(),
            backend: self.backend,
            slot,
            valid: lease.valid_rect(),
            lease: Some(lease),
        }))
    }

    /// Imports the slot of `lease`.
    fn import(&self, lease: &GpuFrame) -> Result<Slot, ImportError> {
        let format = match lease.format() {
            TextureFormat::Rgba8 => wgpu::TextureFormat::Rgba8Unorm,
            _ => wgpu::TextureFormat::Bgra8Unorm,
        };
        let srgb = format.add_srgb_suffix();
        let view_formats = [srgb];
        let desc = wgpu::TextureDescriptor {
            label: Some("kurogane frame"),
            size: wgpu::Extent3d {
                width: lease.width(),
                height: lease.height(),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &view_formats,
        };
        let texture = self.backend.import(&self.device, lease, &desc)?;
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let srgb_view = texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(srgb),
            ..Default::default()
        });
        Ok(Slot {
            id: lease.slot(),
            texture,
            view,
            srgb_view,
        })
    }
}

/// Frame leased as a texture of the host's device.
///
/// Dropped after the last submission that reads it, the frame returns to its
/// ring once the queue has finished the work submitted so far. The queue's next
/// submit or poll finds that work done.
pub struct Frame {
    device: wgpu::Device,
    queue: wgpu::Queue,
    backend: Backend,
    slot: Slot,
    valid: PixelRect,
    /// Held until the queue's work on the frame is done
    lease: Option<GpuFrame>,
}

impl Frame {
    /// Name of the frame's slot, which stays with its texture. A host's bind
    /// groups can key on it.
    pub fn slot(&self) -> SlotId {
        self.slot.id
    }

    /// Texture of the frame. Only read it; the ring owns its pixels.
    pub fn texture(&self) -> &wgpu::Texture {
        &self.slot.texture
    }

    /// View of the frame's pixels as CEF gives them, sRGB-encoded with
    /// premultiplied alpha, for a host that composes in sRGB.
    pub fn view(&self) -> &wgpu::TextureView {
        &self.slot.view
    }

    /// View of the frame that decodes sRGB when sampled, for a host that
    /// composes in linear light.
    pub fn srgb_view(&self) -> &wgpu::TextureView {
        &self.slot.srgb_view
    }

    /// Part of the texture that holds the page.
    pub fn valid_rect(&self) -> PixelRect {
        self.valid
    }
}

impl Drop for Frame {
    fn drop(&mut self) {
        self.backend
            .release(&self.device, &self.queue, &self.slot.texture);
        let lease = self.lease.take();
        self.queue.on_submitted_work_done(move || drop(lease));
    }
}

//! Frames an offscreen browser lends its application.
//!
//! A frame lives for one call of the callback it is lent to. Pixels are a
//! borrowed slice. A shared texture is CEF's own, back in its pool once the
//! callback returns; copy it inside the callback to keep it.

use std::fmt;
use std::time::Duration;

use tanso::{AcceleratedPaintInfo, ColorType, Rect};

/// What an offscreen browser whose popups come as their own layer delivers,
/// in order, for
/// [`OffscreenOptions::on_pixel_layers`](crate::OffscreenOptions::on_pixel_layers)
/// and
/// [`OffscreenOptions::on_shared_texture`](crate::OffscreenOptions::on_shared_texture).
#[derive(Clone, Copy, Debug)]
pub enum Layer<F> {
    /// A frame of the page.
    View(F),
    /// A frame of the popup the page shows (a `<select>`'s for one) with its
    /// place over the view in pixels.
    Popup { frame: F, place: PixelRect },
    /// The popup is gone; the view shows alone.
    PopupHidden,
}

/// Frame of an offscreen browser in pixels, lent for one call of
/// [`OffscreenOptions::on_pixels`](crate::OffscreenOptions::on_pixels) or
/// [`OffscreenOptions::on_pixel_layers`](crate::OffscreenOptions::on_pixel_layers).
#[derive(Clone, Copy)]
pub struct PixelFrame<'a> {
    pub(super) bytes: &'a [u8],
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) damage: &'a [PixelRect],
}

impl<'a> PixelFrame<'a> {
    /// Pixels as BGRA with premultiplied alpha, rows top to bottom without
    /// padding.
    pub fn bytes(&self) -> &'a [u8] {
        self.bytes
    }

    /// Width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Bytes from the start of one row to the next.
    pub fn stride(&self) -> usize {
        self.width as usize * 4
    }

    /// Regions changed since the previous frame. The frame is whole either way.
    pub fn damage(&self) -> &'a [PixelRect] {
        self.damage
    }
}

// By hand: the pixels would flood a log
impl fmt::Debug for PixelFrame<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PixelFrame")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("damage", &self.damage)
            .finish_non_exhaustive()
    }
}

/// Frame of an offscreen browser in a GPU texture CEF owns, lent for one call
/// of [`OffscreenOptions::on_shared_texture`](crate::OffscreenOptions::on_shared_texture).
///
/// The texture is CEF's and goes back to its pool when the callback returns.
/// Open it and copy what is needed inside the callback; the lifetime makes
/// holding the frame past it a compile error.
#[derive(Clone, Copy)]
pub struct SharedTextureFrame<'a> {
    info: &'a AcceleratedPaintInfo,
    damage: &'a [PixelRect],
    #[cfg(target_os = "windows")]
    handle: std::os::windows::io::BorrowedHandle<'a>,
    #[cfg(target_os = "macos")]
    surface: tanso::osr::BorrowedIoSurface<'a>,
}

impl<'a> SharedTextureFrame<'a> {
    /// The frame CEF lends as `info` with `damage`, None when it holds no
    /// texture.
    ///
    /// # Safety
    ///
    /// `info` is the one CEF passed to the running `on_accelerated_paint`.
    pub(super) unsafe fn new(
        info: &'a AcceleratedPaintInfo,
        damage: &'a [PixelRect],
    ) -> Option<Self> {
        #[cfg(target_os = "linux")]
        if info.planes().is_empty() {
            return None;
        }
        Some(Self {
            info,
            damage,
            // SAFETY: the caller's promise
            #[cfg(target_os = "windows")]
            handle: unsafe { info.shared_texture() }?,
            // SAFETY: the caller's promise
            #[cfg(target_os = "macos")]
            surface: unsafe { info.io_surface() }?,
        })
    }

    /// Width of the texture in pixels.
    pub fn width(&self) -> u32 {
        u32::try_from(self.info.extra.coded_size.width).unwrap_or(0)
    }

    /// Height of the texture in pixels.
    pub fn height(&self) -> u32 {
        u32::try_from(self.info.extra.coded_size.height).unwrap_or(0)
    }

    /// Part of the texture that holds the frame.
    pub fn visible_rect(&self) -> PixelRect {
        PixelRect::from_cef(&self.info.extra.visible_rect)
    }

    /// Layout of the texture's pixels.
    pub fn format(&self) -> TextureFormat {
        if self.info.format == ColorType::RGBA_8888 {
            TextureFormat::Rgba8
        } else {
            TextureFormat::Bgra8
        }
    }

    /// Regions changed since the previous frame. The texture holds the whole
    /// frame either way.
    pub fn damage(&self) -> &'a [PixelRect] {
        self.damage
    }

    /// Capture number of the frame, one more than the previous frame's when
    /// none was skipped. None when CEF gives none.
    pub fn capture_counter(&self) -> Option<u64> {
        (self.info.extra.has_capture_counter != 0).then_some(self.info.extra.capture_counter)
    }

    /// When the frame was captured, from the start of capture.
    pub fn timestamp(&self) -> Duration {
        Duration::from_micros(self.info.extra.timestamp)
    }

    /// Windows NT handle of the shared D3D11 texture, without a keyed mutex.
    /// Open it with `ID3D11Device1::OpenSharedResource1`, or a D3D12 or
    /// Vulkan equivalent.
    #[cfg(target_os = "windows")]
    pub fn handle(&self) -> std::os::windows::io::BorrowedHandle<'a> {
        self.handle
    }

    /// macOS IOSurface of the frame.
    #[cfg(target_os = "macos")]
    pub fn io_surface(&self) -> tanso::osr::BorrowedIoSurface<'a> {
        self.surface
    }

    /// Linux dmabuf planes of the frame, in order.
    #[cfg(target_os = "linux")]
    pub fn planes(&self) -> impl Iterator<Item = DmabufPlane<'a>> + 'a {
        self.info.planes().iter().filter_map(|plane| {
            // SAFETY: the planes belong to the info CEF passed to the running
            // on_accelerated_paint, as new's caller promised
            let fd = unsafe { plane.fd() }?;
            Some(DmabufPlane {
                fd,
                stride: plane.stride,
                offset: plane.offset,
                size: plane.size,
            })
        })
    }

    /// Linux DRM format modifier of the frame's dmabuf.
    #[cfg(target_os = "linux")]
    pub fn modifier(&self) -> u64 {
        self.info.modifier
    }
}

// By hand: the texture's handles are CEF's
impl fmt::Debug for SharedTextureFrame<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SharedTextureFrame")
            .field("width", &self.width())
            .field("height", &self.height())
            .field("format", &self.format())
            .field("damage", &self.damage)
            .finish_non_exhaustive()
    }
}

/// Layout of a shared texture's pixels, 8 bits a channel, premultiplied alpha.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TextureFormat {
    Bgra8,
    Rgba8,
}

/// Linux plane of a shared texture's dmabuf, lent with its frame.
#[cfg(target_os = "linux")]
#[derive(Clone, Copy, Debug)]
pub struct DmabufPlane<'a> {
    /// File descriptor CEF closes when the callback returns; duplicate it to
    /// keep it.
    pub fd: std::os::fd::BorrowedFd<'a>,
    /// Bytes from one row to the next.
    pub stride: u32,
    /// Byte offset of the plane in the buffer.
    pub offset: u64,
    /// Size of the plane in bytes.
    pub size: u64,
}

/// Rectangle of a frame in pixels from its top-left corner.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct PixelRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl PixelRect {
    /// The rectangle CEF gives as `rect`, a negative part as zero.
    pub(super) fn from_cef(rect: &Rect) -> Self {
        Self {
            x: rect.x.max(0) as u32,
            y: rect.y.max(0) as u32,
            width: rect.width.max(0) as u32,
            height: rect.height.max(0) as u32,
        }
    }
}

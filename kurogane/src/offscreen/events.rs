//! Events an offscreen browser queues for its host.
//!
//! A cursor, tooltip, input purpose or IME area replaces the one of its kind
//! not yet read and joins the back of the queue. Other events are never
//! dropped or merged.

use std::collections::VecDeque;
use std::fmt;
use std::mem::discriminant;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, PoisonError};

use tanso::sys::cef_cursor_type_t as Cef;
use tanso::{CursorInfo, CursorType};
use tracing::error;

use super::frame::PixelRect;

/// What an offscreen browser tells its host, from
/// [`OffscreenBrowser::events`](crate::OffscreenBrowser::events).
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum OffscreenEvent {
    /// The cursor the page shows over the pointer.
    Cursor(MouseCursor),
    /// The tooltip the page shows; None hides it.
    Tooltip(Option<String>),
    /// What the focused field takes, for the host's IME and on-screen
    /// keyboard. [`InputPurpose::None`] when no field has the focus.
    InputPurpose(InputPurpose),
    /// Where the IME's candidates belong in surface pixels, the text being
    /// composed or else the focused field of the main frame.
    ImeArea(PixelRect),
    /// The frame ring stopped for the reason named. No frame follows; the host
    /// can open the browser again with
    /// [`OffscreenOptions::on_pixels`](crate::OffscreenOptions::on_pixels).
    #[cfg(feature = "gpu-frames")]
    GpuFramesFailed(String),
    /// The browser has closed. No event follows.
    Closed,
}

impl OffscreenEvent {
    /// Returns whether a newer event of this kind replaces an unread one.
    fn coalesces(&self) -> bool {
        matches!(
            self,
            Self::Cursor(_) | Self::Tooltip(_) | Self::InputPurpose(_) | Self::ImeArea(_)
        )
    }
}

/// What a focused field takes, named after its `type`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum InputPurpose {
    /// No field has the focus.
    None,
    Text,
    Password,
    Email,
    Number,
    Url,
    Tel,
    Search,
}

/// Cursor a page shows, named as CSS names it.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum MouseCursor {
    Default,
    Crosshair,
    Pointer,
    Text,
    Wait,
    Help,
    EResize,
    NResize,
    NeResize,
    NwResize,
    SResize,
    SeResize,
    SwResize,
    WResize,
    NsResize,
    EwResize,
    NeswResize,
    NwseResize,
    ColResize,
    RowResize,
    AllScroll,
    Move,
    VerticalText,
    Cell,
    ContextMenu,
    Alias,
    Progress,
    NoDrop,
    Copy,
    NotAllowed,
    ZoomIn,
    ZoomOut,
    Grab,
    Grabbing,
    /// No cursor shows.
    Hidden,
    /// An image the page gives.
    Custom(CursorImage),
}

impl MouseCursor {
    /// The cursor CEF names with `kind`, its image in `info` for a custom one.
    pub(crate) fn from_cef(kind: CursorType, info: &CursorInfo) -> Self {
        match *kind.as_ref() {
            Cef::CT_POINTER => Self::Default,
            Cef::CT_CROSS => Self::Crosshair,
            Cef::CT_HAND => Self::Pointer,
            Cef::CT_IBEAM => Self::Text,
            Cef::CT_WAIT => Self::Wait,
            Cef::CT_HELP => Self::Help,
            Cef::CT_EASTRESIZE => Self::EResize,
            Cef::CT_NORTHRESIZE => Self::NResize,
            Cef::CT_NORTHEASTRESIZE => Self::NeResize,
            Cef::CT_NORTHWESTRESIZE => Self::NwResize,
            Cef::CT_SOUTHRESIZE => Self::SResize,
            Cef::CT_SOUTHEASTRESIZE => Self::SeResize,
            Cef::CT_SOUTHWESTRESIZE => Self::SwResize,
            Cef::CT_WESTRESIZE => Self::WResize,
            Cef::CT_NORTHSOUTHRESIZE => Self::NsResize,
            Cef::CT_EASTWESTRESIZE => Self::EwResize,
            Cef::CT_NORTHEASTSOUTHWESTRESIZE => Self::NeswResize,
            Cef::CT_NORTHWESTSOUTHEASTRESIZE => Self::NwseResize,
            Cef::CT_COLUMNRESIZE => Self::ColResize,
            Cef::CT_ROWRESIZE => Self::RowResize,
            Cef::CT_MIDDLEPANNING
            | Cef::CT_EASTPANNING
            | Cef::CT_NORTHPANNING
            | Cef::CT_NORTHEASTPANNING
            | Cef::CT_NORTHWESTPANNING
            | Cef::CT_SOUTHPANNING
            | Cef::CT_SOUTHEASTPANNING
            | Cef::CT_SOUTHWESTPANNING
            | Cef::CT_WESTPANNING
            | Cef::CT_MIDDLE_PANNING_VERTICAL
            | Cef::CT_MIDDLE_PANNING_HORIZONTAL => Self::AllScroll,
            Cef::CT_MOVE | Cef::CT_DND_MOVE => Self::Move,
            Cef::CT_VERTICALTEXT => Self::VerticalText,
            Cef::CT_CELL => Self::Cell,
            Cef::CT_CONTEXTMENU => Self::ContextMenu,
            Cef::CT_ALIAS | Cef::CT_DND_LINK => Self::Alias,
            Cef::CT_PROGRESS => Self::Progress,
            Cef::CT_NODROP | Cef::CT_DND_NONE => Self::NoDrop,
            Cef::CT_COPY | Cef::CT_DND_COPY => Self::Copy,
            Cef::CT_NONE => Self::Hidden,
            Cef::CT_NOTALLOWED => Self::NotAllowed,
            Cef::CT_ZOOMIN => Self::ZoomIn,
            Cef::CT_ZOOMOUT => Self::ZoomOut,
            Cef::CT_GRAB => Self::Grab,
            Cef::CT_GRABBING => Self::Grabbing,
            Cef::CT_CUSTOM => CursorImage::from_cef(info).map_or(Self::Default, Self::Custom),
            _ => Self::Default,
        }
    }
}

/// Image of a custom cursor.
#[derive(Clone, PartialEq)]
pub struct CursorImage {
    width: u32,
    height: u32,
    hotspot: (u32, u32),
    scale: f32,
    bgra: Vec<u8>,
}

impl CursorImage {
    /// The image CEF lends in `info`, None when it is empty.
    fn from_cef(info: &CursorInfo) -> Option<Self> {
        let width = u32::try_from(info.size.width).ok()?;
        let height = u32::try_from(info.size.height).ok()?;
        let len = (width as usize)
            .checked_mul(height as usize)?
            .checked_mul(4)?;
        if len == 0 || info.buffer.is_null() {
            return None;
        }
        // SAFETY: CEF's cursor buffer is size.width * size.height * 4 bytes
        // of BGRA, valid while the callback runs (cef_types.h, cef_cursor_info_t)
        let bgra = unsafe { std::slice::from_raw_parts(info.buffer.cast::<u8>(), len) };
        Some(Self {
            width,
            height,
            hotspot: (
                u32::try_from(info.hotspot.x).unwrap_or(0),
                u32::try_from(info.hotspot.y).unwrap_or(0),
            ),
            scale: info.image_scale_factor,
            bgra: bgra.to_vec(),
        })
    }

    /// Width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Point of the image that marks the pointer, in pixels from its top-left.
    pub fn hotspot(&self) -> (u32, u32) {
        self.hotspot
    }

    /// Device scale factor the image is drawn for.
    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// Pixels as BGRA with premultiplied alpha, rows top to bottom.
    pub fn bgra(&self) -> &[u8] {
        &self.bgra
    }
}

// By hand: the pixels would flood a log
impl fmt::Debug for CursorImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CursorImage")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("hotspot", &self.hotspot)
            .field("scale", &self.scale)
            .finish_non_exhaustive()
    }
}

/// Wake the host gives with [`OffscreenOptions::wake`](crate::OffscreenOptions::wake).
pub(crate) type Wake = Arc<dyn Fn() + Send + Sync>;

/// Queue of events from the browser's handlers to its host.
pub(crate) struct EventQueue {
    queue: Mutex<VecDeque<OffscreenEvent>>,
    wake: Option<Wake>,
}

impl EventQueue {
    /// Empty queue that runs `wake` after each event it takes.
    pub(crate) fn new(wake: Option<Wake>) -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
            wake,
        }
    }

    /// Queues `event` and wakes the host.
    pub(crate) fn push(&self, event: OffscreenEvent) {
        {
            let mut queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
            if event.coalesces() {
                let kind = discriminant(&event);
                queue.retain(|queued| discriminant(queued) != kind);
            }
            queue.push_back(event);
        }
        self.wake();
    }

    /// Runs the host's wake.
    pub(crate) fn wake(&self) {
        if let Some(wake) = &self.wake
            && catch_unwind(AssertUnwindSafe(|| wake())).is_err()
        {
            error!("an offscreen browser's wake panicked");
        }
    }

    /// Takes every queued event, oldest first.
    pub(crate) fn drain(&self) -> OffscreenEvents {
        let mut queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
        OffscreenEvents(std::mem::take(&mut *queue).into_iter())
    }
}

/// Events taken from an offscreen browser, oldest first, from
/// [`OffscreenBrowser::events`](crate::OffscreenBrowser::events).
#[derive(Debug)]
pub struct OffscreenEvents(std::collections::vec_deque::IntoIter<OffscreenEvent>);

impl Iterator for OffscreenEvents {
    type Item = OffscreenEvent;

    fn next(&mut self) -> Option<OffscreenEvent> {
        self.0.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_newer_cursor_replaces_an_unread_one_at_the_back() {
        let queue = EventQueue::new(None);
        queue.push(OffscreenEvent::Cursor(MouseCursor::Text));
        queue.push(OffscreenEvent::Tooltip(Some("tip".into())));
        queue.push(OffscreenEvent::Cursor(MouseCursor::Pointer));
        let events: Vec<_> = queue.drain().collect();
        assert_eq!(
            events,
            [
                OffscreenEvent::Tooltip(Some("tip".into())),
                OffscreenEvent::Cursor(MouseCursor::Pointer),
            ]
        );
    }

    #[test]
    fn other_events_are_kept() {
        let queue = EventQueue::new(None);
        queue.push(OffscreenEvent::Closed);
        queue.push(OffscreenEvent::Closed);
        assert_eq!(queue.drain().count(), 2);
        assert_eq!(queue.drain().count(), 0);
    }
}

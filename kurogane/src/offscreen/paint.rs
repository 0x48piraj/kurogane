//! Frames of offscreen browsers, delivered on CEF's UI thread.
//!
//! The application's frame callbacks need not be `Send`. They live in storage
//! of the UI thread, the only thread that paints, runs them and drops them.
//! The render handler, which CEF may release on any thread, holds their key.

use std::cell::RefCell;
use std::collections::HashMap;
use std::os::raw::c_int;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use tanso::*;
use tracing::{debug, error, warn};

use super::{Layer, OffscreenEvent, PixelFrame, PixelRect, Shared, SharedTextureFrame};

/// Frame callback the application gives with
/// [`OffscreenOptions::on_pixels`](crate::OffscreenOptions::on_pixels).
pub(crate) type OnPixels = Box<dyn FnMut(PixelFrame<'_>)>;

/// Frame callback the application gives with
/// [`OffscreenOptions::on_pixel_layers`](crate::OffscreenOptions::on_pixel_layers).
pub(crate) type OnPixelLayers = Box<dyn FnMut(Layer<PixelFrame<'_>>)>;

/// Frame callback the application gives with
/// [`OffscreenOptions::on_shared_texture`](crate::OffscreenOptions::on_shared_texture).
pub(crate) type OnSharedTexture = Box<dyn FnMut(Layer<SharedTextureFrame<'_>>)>;

thread_local! {
    /// Painters of this thread's offscreen browsers. Only CEF's UI thread has any.
    static PAINTERS: RefCell<Painters> = RefCell::new(Painters::default());
}

/// Key of an offscreen browser's painter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PainterKey(u64);

#[derive(Default)]
struct Painters {
    next: u64,
    /// None while its painter is out delivering a frame
    slots: HashMap<PainterKey, Option<Painter>>,
}

/// Keeps `painter` on this thread and returns its key.
pub(crate) fn keep(painter: Painter) -> PainterKey {
    PAINTERS.with_borrow_mut(|painters| {
        painters.next += 1;
        let key = PainterKey(painters.next);
        painters.slots.insert(key, Some(painter));
        key
    })
}

/// Drops the painter of `key`, its callbacks with it.
pub(crate) fn drop_painter(key: PainterKey) {
    // Dropped outside the borrow; a callback's drop may reach this storage
    let painter = PAINTERS.with_borrow_mut(|painters| painters.slots.remove(&key));
    drop(painter);
}

/// Runs `f` on the painter of `key`, taken out of the storage while `f` runs
/// application code. Does nothing for a painter already out or gone.
fn with_painter<R>(key: PainterKey, f: impl FnOnce(&mut Painter) -> R) -> Option<R> {
    let mut painter = PAINTERS.with_borrow_mut(|painters| painters.slots.get_mut(&key)?.take())?;
    let result = f(&mut painter);
    // A painter dropped meanwhile stays dropped
    let left = PAINTERS.with_borrow_mut(|painters| match painters.slots.get_mut(&key) {
        Some(slot) => {
            *slot = Some(painter);
            None
        }
        None => Some(painter),
    });
    drop(left);
    Some(result)
}

/// Frame callbacks and buffers of one offscreen browser.
pub(crate) struct Painter {
    /// Whole frames, a popup drawn in
    pixels: Option<OnPixels>,
    /// Pixel frames, a popup a layer of its own
    pixel_layers: Option<OnPixelLayers>,
    /// CEF's textures, a popup a layer of its own
    texture: Option<OnSharedTexture>,
    /// Damage of the frame being delivered, kept to spare an allocation each frame
    damage: Vec<PixelRect>,
    /// The popup, composed into whole frames
    popup: Popup,
    /// The popup's place over the view in pixels, for the layers
    place: PixelRect,
    /// A software frame has come to a browser that takes textures alone
    software_dropped: bool,
}

impl Painter {
    /// Painter delivering frames to the callbacks given.
    pub(crate) fn new(
        pixels: Option<OnPixels>,
        pixel_layers: Option<OnPixelLayers>,
        texture: Option<OnSharedTexture>,
    ) -> Self {
        Self {
            pixels,
            pixel_layers,
            texture,
            damage: Vec::new(),
            popup: Popup::default(),
            place: PixelRect::default(),
            software_dropped: false,
        }
    }

    /// Takes a frame in pixels of the view or of its popup.
    fn paint(&mut self, element: PaintElementType, dirty: &[Rect], frame: Pixels<'_>) {
        if self.pixels.is_some() {
            self.paint_whole(element, dirty, frame);
        } else if self.pixel_layers.is_some() {
            self.damage.clear();
            self.damage.extend(dirty.iter().map(PixelRect::from_cef));
            let frame = frame.with_damage(&self.damage);
            let layer = if element == PaintElementType::POPUP {
                Layer::Popup {
                    frame,
                    place: self.place,
                }
            } else {
                Layer::View(frame)
            };
            guarded(&mut self.pixel_layers, "on_pixel_layers", |f| f(layer));
        } else if !self.software_dropped {
            self.software_dropped = true;
            warn!(
                "an offscreen browser that takes shared textures alone got a frame Chromium composited in software; it drops such frames"
            );
        }
    }

    /// Takes a frame in CEF's texture of the view or of its popup.
    fn paint_texture(
        &mut self,
        element: PaintElementType,
        dirty: &[Rect],
        info: &AcceleratedPaintInfo,
    ) {
        self.damage.clear();
        self.damage.extend(dirty.iter().map(PixelRect::from_cef));
        // SAFETY: info is the one CEF passed to the running on_accelerated_paint
        let Some(frame) = (unsafe { SharedTextureFrame::new(info, &self.damage) }) else {
            debug!("an offscreen frame came without its texture");
            return;
        };
        let layer = if element == PaintElementType::POPUP {
            Layer::Popup {
                frame,
                place: self.place,
            }
        } else {
            Layer::View(frame)
        };
        guarded(&mut self.texture, "on_shared_texture", |f| f(layer));
    }

    /// Delivers a whole frame of the view, the popup drawn in while it shows.
    fn paint_whole(&mut self, element: PaintElementType, dirty: &[Rect], frame: Pixels<'_>) {
        if element == PaintElementType::POPUP {
            self.popup.pixels.copy_from(frame);
            self.deliver_composed();
            return;
        }
        if !self.popup.shown {
            self.damage.clear();
            self.damage.extend(dirty.iter().map(PixelRect::from_cef));
            let frame = frame.with_damage(&self.damage);
            guarded(&mut self.pixels, "on_pixels", |f| f(frame));
            return;
        }
        self.popup.view.copy_from(frame);
        self.deliver_composed();
    }

    /// Delivers the view with the popup drawn over it once the popup has a
    /// frame, the view alone before.
    fn deliver_composed(&mut self) {
        let popup = &mut self.popup;
        if !popup.shown || popup.view.is_empty() {
            return;
        }
        popup.composed.clear();
        popup.composed.extend_from_slice(&popup.view.bytes);
        if !popup.pixels.is_empty() {
            blit(&mut popup.composed, &popup.view, &popup.pixels, popup.place);
        }
        let view = Pixels {
            bytes: &popup.composed,
            width: popup.view.width,
            height: popup.view.height,
        };
        self.damage.clear();
        self.damage.push(view.whole());
        let frame = view.with_damage(&self.damage);
        guarded(&mut self.pixels, "on_pixels", |f| f(frame));
    }

    /// Shows or hides the popup. Returns whether whole frames need a frame of
    /// the view.
    fn popup_shown(&mut self, shown: bool) -> bool {
        self.popup.shown = shown;
        if !shown {
            guarded(&mut self.pixel_layers, "on_pixel_layers", |f| {
                f(Layer::PopupHidden)
            });
            guarded(&mut self.texture, "on_shared_texture", |f| {
                f(Layer::PopupHidden)
            });
        }
        if self.pixels.is_none() {
            return false;
        }
        if shown {
            return self.popup.view.is_empty();
        }
        // The kept view is the latest; delivered, it takes the popup away
        let view = std::mem::take(&mut self.popup.view);
        self.popup = Popup::default();
        if !view.is_empty() {
            let frame = view.as_pixels();
            self.damage.clear();
            self.damage.push(frame.whole());
            let frame = frame.with_damage(&self.damage);
            guarded(&mut self.pixels, "on_pixels", |f| f(frame));
        }
        false
    }

    /// Places the popup at `place`, view pixels. Returns whether the layers
    /// need a frame of the popup to carry the new place.
    fn popup_placed(&mut self, place: PixelRect) -> bool {
        self.place = place;
        if self.pixels.is_some() {
            self.popup.place = (place.x as i32, place.y as i32);
            self.deliver_composed();
        }
        self.popup.shown && (self.pixel_layers.is_some() || self.texture.is_some())
    }
}

/// Runs one of the application's frame callbacks with `call`; a callback
/// that panics takes no more frames.
fn guarded<C: ?Sized>(callback: &mut Option<Box<C>>, name: &str, call: impl FnOnce(&mut C)) {
    let Some(f) = callback.as_mut() else {
        return;
    };
    if catch_unwind(AssertUnwindSafe(|| call(&mut **f))).is_err() {
        error!("an offscreen browser's {name} panicked; it takes no more frames");
        *callback = None;
    }
}

/// Popup of a `<select>` or picker and the frames composed with it.
#[derive(Default)]
struct Popup {
    shown: bool,
    /// Top-left corner in view pixels
    place: (i32, i32),
    /// Latest frame of the view while the popup shows
    view: Owned,
    /// Latest frame of the popup
    pixels: Owned,
    composed: Vec<u8>,
}

/// BGRA frame lent by CEF.
#[derive(Clone, Copy)]
struct Pixels<'a> {
    bytes: &'a [u8],
    width: u32,
    height: u32,
}

impl<'a> Pixels<'a> {
    /// The frame CEF lends as `buffer` of `width` by `height` pixels, None when
    /// the sizes disagree.
    fn new(buffer: &'a [u8], width: c_int, height: c_int) -> Option<Self> {
        let width = u32::try_from(width).ok()?;
        let height = u32::try_from(height).ok()?;
        let len = (width as usize)
            .checked_mul(height as usize)?
            .checked_mul(4)?;
        (buffer.len() == len).then_some(Self {
            bytes: buffer,
            width,
            height,
        })
    }

    /// Rectangle of the whole frame.
    fn whole(&self) -> PixelRect {
        PixelRect {
            x: 0,
            y: 0,
            width: self.width,
            height: self.height,
        }
    }

    fn with_damage(self, damage: &'a [PixelRect]) -> PixelFrame<'a> {
        PixelFrame {
            bytes: self.bytes,
            width: self.width,
            height: self.height,
            damage,
        }
    }
}

/// BGRA frame kept by Kurogane.
#[derive(Default)]
struct Owned {
    bytes: Vec<u8>,
    width: u32,
    height: u32,
}

impl Owned {
    fn copy_from(&mut self, frame: Pixels<'_>) {
        self.bytes.clear();
        self.bytes.extend_from_slice(frame.bytes);
        self.width = frame.width;
        self.height = frame.height;
    }

    fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    fn as_pixels(&self) -> Pixels<'_> {
        Pixels {
            bytes: &self.bytes,
            width: self.width,
            height: self.height,
        }
    }
}

/// Copies `popup` into `out`, a frame the size of `view`, with its top-left
/// corner at `place`; what falls outside is cut.
fn blit(out: &mut [u8], view: &Owned, popup: &Owned, place: (i32, i32)) {
    let (left, top) = (i64::from(place.0), i64::from(place.1));
    let (view_w, view_h) = (i64::from(view.width), i64::from(view.height));
    let popup_w = i64::from(popup.width);
    let first_x = left.max(0);
    let last_x = (left + popup_w).min(view_w);
    if first_x >= last_x {
        return;
    }
    let row_bytes = ((last_x - first_x) * 4) as usize;
    for row in 0..i64::from(popup.height) {
        let y = top + row;
        if !(0..view_h).contains(&y) {
            continue;
        }
        let from = ((row * popup_w + (first_x - left)) * 4) as usize;
        let to = ((y * view_w + first_x) * 4) as usize;
        out[to..to + row_bytes].copy_from_slice(&popup.bytes[from..from + row_bytes]);
    }
}

// Asks CEF for a fresh frame of the view or the popup, outside the callback
// that needs it
wrap_task! {
    struct Repaint {
        browser: Browser,
        element: PaintElementType,
    }

    impl Task {
        fn execute(&self) {
            if let Some(host) = self.browser.host() {
                host.invalidate(self.element);
            }
        }
    }
}

/// Posts a repaint of `element` of `browser` to the UI thread.
fn repaint_later(browser: &Browser, element: PaintElementType) {
    let mut task = Repaint::new(browser.clone(), element);
    if post_task(ThreadId::UI, &mut task) == 0 {
        debug!("CEF refused a repaint of an offscreen browser");
    }
}

wrap_render_handler! {
    pub(crate) struct OffscreenRenderHandler {
        key: PainterKey,
        shared: Arc<Shared>,
    }

    impl RenderHandler {
        fn view_rect(&self, _browser: &mut Browser, rect: &mut Rect) {
            let (width, height) = self.shared.view().dip_size();
            *rect = Rect { x: 0, y: 0, width, height };
        }

        // The page's window on the screen, which its screenX and screenY read
        fn root_screen_rect(&self, _browser: &mut Browser, rect: &mut Rect) -> c_int {
            let (x, y, width, height) = self.shared.view().screen_rect();
            *rect = Rect { x, y, width, height };
            1
        }

        fn screen_point(
            &self,
            _browser: &mut Browser,
            view_x: c_int,
            view_y: c_int,
            screen_x: &mut c_int,
            screen_y: &mut c_int,
        ) -> c_int {
            (*screen_x, *screen_y) = self.shared.view().screen_point(view_x, view_y);
            1
        }

        // The screen is the view, which keeps popups inside the frame
        fn screen_info(&self, _browser: &mut Browser, screen_info: &mut ScreenInfo) -> c_int {
            let view = self.shared.view();
            let (width, height) = view.dip_size();
            let rect = Rect { x: 0, y: 0, width, height };
            screen_info.device_scale_factor = view.scale() as f32;
            screen_info.depth = 24;
            screen_info.depth_per_component = 8;
            screen_info.is_monochrome = 0;
            screen_info.rect = rect;
            screen_info.available_rect = rect;
            1
        }

        fn on_popup_show(&self, browser: &mut Browser, show: c_int) {
            let needs_view = with_painter(self.key, |painter| painter.popup_shown(show != 0));
            if needs_view == Some(true) {
                repaint_later(browser, PaintElementType::VIEW);
            }
        }

        // The rectangle is in view DIPs and may reach past the view
        fn on_popup_size(&self, browser: &mut Browser, rect: &Rect) {
            let place = pixels_of(rect, self.shared.view().scale());
            let needs_popup = with_painter(self.key, |painter| painter.popup_placed(place));
            if needs_popup == Some(true) {
                repaint_later(browser, PaintElementType::POPUP);
            }
        }

        fn start_dragging(
            &self,
            browser: &mut Browser,
            drag_data: &mut DragData,
            allowed_ops: DragOperationsMask,
            _x: c_int,
            _y: c_int,
        ) -> c_int {
            let event = self.shared.pointer(|pointer, scale| pointer.event(scale));
            super::drag::start(self.key, browser, drag_data, allowed_ops, super::mouse_event(event)).into()
        }

        fn update_drag_cursor(&self, _browser: &mut Browser, operation: DragOperationsMask) {
            super::drag::accept(self.key, operation);
        }

        // The characters' bounds are in view DIPs
        fn on_ime_composition_range_changed(
            &self,
            _browser: &mut Browser,
            _selected_range: &Range,
            character_bounds: &[Rect],
        ) {
            let Some(area) = character_bounds.iter().copied().reduce(union) else {
                return;
            };
            let area = pixels_of(&area, self.shared.view().scale());
            self.shared.events.push(OffscreenEvent::ImeArea(area));
        }

        fn on_paint(
            &self,
            _browser: &mut Browser,
            type_: PaintElementType,
            dirty_rects: &[Rect],
            buffer: &[u8],
            width: c_int,
            height: c_int,
        ) {
            let Some(frame) = Pixels::new(buffer, width, height) else {
                debug!("an offscreen frame of {width}x{height} came with {} bytes", buffer.len());
                return;
            };
            with_painter(self.key, |painter| painter.paint(type_, dirty_rects, frame));
        }

        fn on_accelerated_paint(
            &self,
            _browser: &mut Browser,
            type_: PaintElementType,
            dirty_rects: &[Rect],
            info: &AcceleratedPaintInfo,
        ) {
            with_painter(self.key, |painter| painter.paint_texture(type_, dirty_rects, info));
        }
    }
}

/// Returns the pixels covering `rect` of view DIPs at `scale`.
fn pixels_of(rect: &Rect, scale: f64) -> PixelRect {
    let scaled = |dips: c_int, round: fn(f64) -> f64| round(f64::from(dips) * scale) as c_int;
    PixelRect::from_cef(&Rect {
        x: scaled(rect.x, f64::floor),
        y: scaled(rect.y, f64::floor),
        width: scaled(rect.width, f64::ceil),
        height: scaled(rect.height, f64::ceil),
    })
}

/// Returns the smallest rectangle holding `a` and `b`.
fn union(a: Rect, b: Rect) -> Rect {
    let (left, top) = (a.x.min(b.x), a.y.min(b.y));
    let right = (a.x + a.width).max(b.x + b.width);
    let bottom = (a.y + a.height).max(b.y + b.height);
    Rect {
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
    }
}

/// Render handler of the offscreen browser whose painter is `key`.
pub(crate) fn render_handler(key: PainterKey, shared: Arc<Shared>) -> RenderHandler {
    OffscreenRenderHandler::new(key, shared)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(width: u32, height: u32, value: u8) -> Owned {
        Owned {
            bytes: vec![value; (width * height * 4) as usize],
            width,
            height,
        }
    }

    #[test]
    fn a_popup_is_drawn_at_its_place_and_cut_at_the_edge() {
        let view = frame(4, 3, 0);
        let popup = frame(2, 2, 9);
        let mut out = view.bytes.clone();
        blit(&mut out, &view, &popup, (3, 2));
        let painted: Vec<usize> = out
            .chunks(4)
            .enumerate()
            .filter(|(_, px)| px[0] == 9)
            .map(|(i, _)| i)
            .collect();
        // Only the pixel at (3, 2) lies inside the view
        assert_eq!(painted, [3 + 2 * 4]);
    }

    #[test]
    fn a_moved_popup_asks_for_a_frame_only_where_layers_take_it() {
        let mut layered = Painter::new(
            Some(Box::new(|_| {})),
            None,
            Some(Box::new(|_: Layer<SharedTextureFrame<'_>>| {})),
        );
        layered.popup_shown(true);
        assert!(layered.popup_placed(PixelRect::default()));

        let mut composed = Painter::new(Some(Box::new(|_| {})), None, None);
        composed.popup_shown(true);
        assert!(!composed.popup_placed(PixelRect::default()));
    }
}

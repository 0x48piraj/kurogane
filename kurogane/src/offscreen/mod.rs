//! Offscreen browsers, whose frames go to the application.
//!
//! An offscreen browser has no window. The application draws its frames,
//! sizes it, shows or hides it and gives it the user's input. Its cursor,
//! tooltip and what its IME needs come as events.

mod drag;
#[cfg(target_os = "macos")]
pub(crate) mod editing;
mod events;
pub(crate) mod field;
mod frame;
mod input;
mod paint;
mod typing;
mod view;

pub use events::{CursorImage, InputPurpose, MouseCursor, OffscreenEvent, OffscreenEvents};
#[cfg(target_os = "linux")]
pub use frame::DmabufPlane;
pub use frame::{Layer, PixelFrame, PixelRect, SharedTextureFrame, TextureFormat};
pub use input::{PointerButton, WheelDelta};
pub use typing::KeyInput;

use std::fmt;
use std::os::raw::c_int;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use tanso::*;
use tracing::debug;

use crate::browser_registry::{BrowserId, BrowserType};
use crate::client::{KuroganeClient, Surface};
use crate::error::RuntimeError;
use crate::keys::Modifiers;
use crate::runtime::{AppHandle, BrowserHandle};
use events::{EventQueue, Wake};
use input::{DoubleClick, Pointer, PointerEvent};
use paint::{OnPixelLayers, OnPixels, OnSharedTexture, Painter, PainterKey};
use typing::NO_RANGE;
use view::View;

/// Frames a second Chromium's capturer takes at most.
const MAX_FRAME_RATE: c_int = 1000;

/// `windowless_frame_rate` under host pacing. CEF's capture period caps
/// external begin frames too; at the capturer's limit it never does.
const HOST_FRAME_RATE: c_int = MAX_FRAME_RATE;

/// CEF's colours are ARGB.
const WHITE: u32 = 0xFFFF_FFFF;
const TRANSPARENT: u32 = 0x0000_0000;

/// How an offscreen browser's frames are timed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pacing {
    /// Chromium's own clock at the display's refresh rate in hertz. An idle
    /// page sends no frame. The clock keeps a phase of its own and whole
    /// rates. Its frames drift against the host's vsync; [`Pacing::Host`]
    /// keeps them in step.
    Display { hz: f64 },
    /// A frame for each [`OffscreenBrowser::begin_frame`], which the host calls
    /// once a vsync. Frames stay in step with the host's vsync.
    ///
    /// A known CEF issue at CEF 156: a crash of Chromium's GPU process can
    /// stop such a browser's frames for good. Its start can rarely freeze the
    /// same way. A browser created again paints.
    Host,
}

impl Default for Pacing {
    fn default() -> Self {
        Self::Display { hz: 60.0 }
    }
}

/// How an offscreen browser opens, for
/// [`AppInstance::create_offscreen_browser`](crate::AppInstance::create_offscreen_browser).
pub struct OffscreenOptions {
    width: u32,
    height: u32,
    scale: f64,
    pacing: Pacing,
    transparent: bool,
    pixels: Option<OnPixels>,
    pixel_layers: Option<OnPixelLayers>,
    texture: Option<OnSharedTexture>,
    wake: Option<Wake>,
}

impl OffscreenOptions {
    /// Options for a surface of `width` by `height` pixels at the device scale
    /// factor `scale`.
    ///
    /// The page lays out in whole DIPs, the smallest size whose frames cover
    /// the surface. At a fractional scale a frame can be a pixel or two wider
    /// or taller than the surface. A surface without area hides the browser.
    pub fn new(width: u32, height: u32, scale: f64) -> Self {
        Self {
            width,
            height,
            scale,
            pacing: Pacing::default(),
            transparent: false,
            pixels: None,
            pixel_layers: None,
            texture: None,
            wake: None,
        }
    }

    /// How frames are timed; [`Pacing::Display`] at 60 Hz by default.
    pub fn pacing(mut self, pacing: Pacing) -> Self {
        self.pacing = pacing;
        self
    }

    /// Leaves what the page does not paint transparent instead of white.
    pub fn transparent(mut self) -> Self {
        self.transparent = true;
        self
    }

    /// Takes each frame as pixels, a popup the page opens (a `<select>`'s for
    /// one) drawn into it.
    ///
    /// `f` runs on the thread that started the application, CEF's UI thread,
    /// inside [`AppInstance::pump`](crate::AppInstance::pump) for an
    /// application that pumps. It may hold what cannot leave that thread, a
    /// texture of the host's for one. Takes the place of
    /// [`on_pixel_layers`](Self::on_pixel_layers).
    pub fn on_pixels<F>(mut self, f: F) -> Self
    where
        F: FnMut(PixelFrame<'_>) + 'static,
    {
        self.pixels = Some(Box::new(f));
        self
    }

    /// Takes each frame as pixels, a popup the page opens as a layer of its
    /// own for the host to draw over the view.
    ///
    /// `f` runs as [`on_pixels`](Self::on_pixels)' does and gets the view's
    /// frames, the popup's frames with their place and the popup's end, in
    /// order. Takes the place of [`on_pixels`](Self::on_pixels).
    pub fn on_pixel_layers<F>(mut self, f: F) -> Self
    where
        F: FnMut(Layer<PixelFrame<'_>>) + 'static,
    {
        self.pixel_layers = Some(Box::new(f));
        self
    }

    /// Takes each frame as CEF's GPU texture while Chromium composites on the
    /// GPU, a popup the page opens as a layer of its own.
    ///
    /// `f` runs as [`on_pixels`](Self::on_pixels)' does. Chromium composites
    /// in software when its GPU is blocked or turned off. Those frames go to
    /// [`on_pixels`](Self::on_pixels) or
    /// [`on_pixel_layers`](Self::on_pixel_layers) when one is given; without
    /// either they are dropped.
    pub fn on_shared_texture<F>(mut self, f: F) -> Self
    where
        F: FnMut(Layer<SharedTextureFrame<'_>>) + 'static,
    {
        self.texture = Some(Box::new(f));
        self
    }

    /// Runs `f` whenever an event waits in [`OffscreenBrowser::events`].
    ///
    /// `f` may run on any thread. It returns at once, after waking the host's
    /// loop for one.
    pub fn wake<F>(mut self, f: F) -> Self
    where
        F: Fn() + Send + Sync + 'static,
    {
        self.wake = Some(Arc::new(f));
        self
    }

    /// Returns what no offscreen browser can have, None for valid options.
    fn problem(&self) -> Option<&'static str> {
        if self.pixels.is_some() && self.pixel_layers.is_some() {
            return Some(
                "an offscreen browser takes pixels through on_pixels or on_pixel_layers, not both",
            );
        }
        if self.pixels.is_none() && self.pixel_layers.is_none() && self.texture.is_none() {
            return Some(
                "an offscreen browser takes its frames through on_pixels, on_pixel_layers or on_shared_texture",
            );
        }
        if !(self.scale.is_finite() && self.scale > 0.0) {
            return Some("the scale is a positive finite number");
        }
        if let Pacing::Display { hz } = self.pacing
            && !(hz.is_finite() && hz > 0.0)
        {
            return Some("the refresh rate is a positive finite number");
        }
        None
    }
}

// By hand: the callbacks have no Debug
impl fmt::Debug for OffscreenOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OffscreenOptions")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("scale", &self.scale)
            .field("pacing", &self.pacing)
            .field("transparent", &self.transparent)
            .field("on_pixels", &self.pixels.is_some())
            .field("on_pixel_layers", &self.pixel_layers.is_some())
            .field("on_shared_texture", &self.texture.is_some())
            .field("wake", &self.wake.is_some())
            .finish()
    }
}

/// State an offscreen browser's handle shares with its handlers.
pub(crate) struct Shared {
    view: Mutex<View>,
    pointer: Mutex<Pointer>,
    pacing: Pacing,
    events: EventQueue,
}

impl Shared {
    /// The view as it stands.
    fn view(&self) -> View {
        *self.view.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Changes the view with `f` and returns it before and after.
    fn change(&self, f: impl FnOnce(&mut View)) -> (View, View) {
        let mut view = self.view.lock().unwrap_or_else(PoisonError::into_inner);
        let before = *view;
        f(&mut view);
        (before, *view)
    }

    /// Runs `f` on the pointer with the view's scale.
    fn pointer<T>(&self, f: impl FnOnce(&mut Pointer, f64) -> T) -> T {
        let scale = self.view().scale();
        let mut pointer = self.pointer.lock().unwrap_or_else(PoisonError::into_inner);
        f(&mut pointer, scale)
    }
}

/// What the client of an offscreen browser holds of it.
#[derive(Clone)]
pub(crate) struct OffscreenClient {
    key: PainterKey,
    shared: Arc<Shared>,
    render: RenderHandler,
}

impl OffscreenClient {
    /// The browser's render handler, the same one each time CEF asks.
    pub(crate) fn render_handler(&self) -> RenderHandler {
        self.render.clone()
    }

    /// Queues `event` for the host.
    pub(crate) fn push(&self, event: OffscreenEvent) {
        self.shared.events.push(event);
    }

    /// Ends the browser's frames and tells the host it closed. UI thread.
    pub(crate) fn closed(&self) {
        paint::drop_painter(self.key);
        drag::forget(self.key);
        self.shared.events.push(OffscreenEvent::Closed);
    }
}

/// Browser without a window, from
/// [`AppInstance::create_offscreen_browser`](crate::AppInstance::create_offscreen_browser).
///
/// Its frames go to the callbacks of its [`OffscreenOptions`]. Its cursor,
/// tooltip and what its IME needs come from [`events`](Self::events). Dropping
/// it closes the browser at once without asking the page;
/// [`close`](Self::close) asks first.
///
/// # Threads
///
/// Usable from any thread. Each call reaches CEF later on its UI thread,
/// never inside a frame callback.
pub struct OffscreenBrowser {
    browser: BrowserHandle,
    shared: Arc<Shared>,
    key: PainterKey,
}

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<OffscreenBrowser>();
};

impl OffscreenBrowser {
    /// The browser for navigation, scripts and DevTools.
    pub fn handle(&self) -> &BrowserHandle {
        &self.browser
    }

    /// Sets the surface's size in pixels. One without area hides the browser
    /// and keeps its layout.
    pub fn set_size(&self, width: u32, height: u32) {
        let (before, after) = self.shared.change(|view| view.set_surface(width, height));
        if after.dip_size() != before.dip_size() {
            self.call(HostCall::Resized);
        }
        if after.shown() != before.shown() {
            self.call(HostCall::Hidden(!after.shown()));
        }
    }

    /// Sets the device scale factor. The page hears it as a change of
    /// `devicePixelRatio`.
    ///
    /// # Panics
    ///
    /// For a scale that is not a positive finite number.
    #[track_caller]
    pub fn set_scale(&self, scale: f64) {
        assert!(
            scale.is_finite() && scale > 0.0,
            "the scale is a positive finite number, not {scale}"
        );
        let (before, after) = self.shared.change(|view| view.set_scale(scale));
        if after != before {
            self.call(HostCall::ScreenChanged);
        }
    }

    /// Sets the surface's top-left corner on the screen in pixels, from which
    /// menus, dialogs and drags place themselves and which the page reads as
    /// `screenX` and `screenY`. The screen's origin by default.
    pub fn set_screen_origin(&self, x: i32, y: i32) {
        let (before, after) = self.shared.change(|view| view.set_origin(x, y));
        if after != before {
            self.call(HostCall::ScreenChanged);
        }
    }

    /// Shows or hides the browser. A hidden browser paints nothing and its
    /// page sees itself hidden.
    pub fn set_visible(&self, visible: bool) {
        let (before, after) = self.shared.change(|view| view.set_visible(visible));
        if after.shown() != before.shown() {
            self.call(HostCall::Hidden(!after.shown()));
        }
    }

    /// Sets the display's refresh rate in hertz under [`Pacing::Display`].
    /// Does nothing under [`Pacing::Host`].
    ///
    /// # Panics
    ///
    /// For a rate that is not a positive finite number.
    #[track_caller]
    pub fn set_refresh_rate(&self, hz: f64) {
        assert!(
            hz.is_finite() && hz > 0.0,
            "the refresh rate is a positive finite number, not {hz}"
        );
        if let Pacing::Display { .. } = self.shared.pacing {
            self.call(HostCall::FrameRate(frame_rate(hz)));
        }
    }

    /// Asks for a frame under [`Pacing::Host`]. Does nothing under
    /// [`Pacing::Display`].
    pub fn begin_frame(&self) {
        if self.shared.pacing == Pacing::Host {
            self.call(HostCall::BeginFrame);
        }
    }

    /// Asks for a whole new frame, for a host that lost its copy.
    pub fn repaint(&self) {
        self.call(HostCall::Repaint);
    }

    /// Asks the browser to close. The page's unload handlers may cancel.
    /// [`OffscreenEvent::Closed`] tells that it closed.
    pub fn close(&self) {
        self.call(HostCall::Close { force: false });
    }

    /// Moves the pointer to `(x, y)` in surface pixels.
    pub fn pointer_move(&self, x: f64, y: f64) {
        let event = self
            .shared
            .pointer(|pointer, scale| pointer.moved(x, y, scale));
        self.call(HostCall::PointerMove {
            event,
            leave: false,
        });
    }

    /// Presses `button` at `(x, y)` in surface pixels. Quick presses in
    /// place count as a double or triple click by the OS's double-click time
    /// and distance.
    pub fn pointer_down(&self, x: f64, y: f64, button: PointerButton) {
        let at = Instant::now();
        let (event, count) = self
            .shared
            .pointer(|pointer, scale| pointer.pressed(x, y, button, scale, at));
        self.call(HostCall::PointerButton {
            event,
            button,
            up: false,
            count,
        });
    }

    /// Releases `button` at `(x, y)` in surface pixels.
    pub fn pointer_up(&self, x: f64, y: f64, button: PointerButton) {
        let (event, count) = self
            .shared
            .pointer(|pointer, scale| pointer.released(x, y, button, scale));
        self.call(HostCall::PointerButton {
            event,
            button,
            up: true,
            count,
        });
    }

    /// Tells the page the pointer left the surface.
    pub fn pointer_leave(&self) {
        let event = self.shared.pointer(|pointer, scale| pointer.event(scale));
        self.call(HostCall::PointerMove { event, leave: true });
    }

    /// Lets go of the held buttons without a click, for a host that lost the
    /// pointer during a press. A drag the page started ends without a drop.
    pub fn pointer_cancel(&self) {
        let event = self
            .shared
            .pointer(|pointer, scale| pointer.cancelled(scale));
        self.call(HostCall::PointerCancel(event));
    }

    /// Turns the wheel by `delta` at the pointer's last position.
    pub fn wheel(&self, delta: WheelDelta) {
        let turn = self
            .shared
            .pointer(|pointer, scale| pointer.wheel(delta, scale));
        if let Some((event, (x, y))) = turn {
            self.call(HostCall::Wheel { event, x, y });
        }
    }

    /// Gives the browser the keyboard focus or takes it away. The page sees
    /// its focus and blur events and shows a caret only while focused.
    pub fn set_focused(&self, focused: bool) {
        self.call(HostCall::Focus(focused));
    }

    /// Sets the modifier keys held and the locks on, which every later
    /// pointer and wheel event carries.
    pub fn set_modifiers(&self, modifiers: Modifiers) {
        self.shared
            .pointer(|pointer, _| pointer.set_modifiers(modifiers));
    }

    /// Presses `key` with the modifiers of [`set_modifiers`](Self::set_modifiers).
    /// Its text types after it, one UTF-16 unit as the key's character and
    /// longer text committed whole.
    pub fn key_down(&self, key: &KeyInput) {
        let modifiers = self.shared.pointer(|pointer, _| pointer.modifiers());
        let press = key.press(modifiers);
        self.call(HostCall::Key(press.down));
        if let Some(character) = press.character {
            self.call(HostCall::Key(character));
        }
        if let Some(text) = press.commit {
            self.call(HostCall::ImeCommit(text));
        }
    }

    /// Releases `key`.
    pub fn key_up(&self, key: &KeyInput) {
        let modifiers = self.shared.pointer(|pointer, _| pointer.modifiers());
        self.call(HostCall::Key(key.release(modifiers)));
    }

    /// Shows `text` as the IME composes it, underlined in the focused field,
    /// the cursor or selection at byte offsets `cursor` into it (its end for
    /// none). An empty text ends the composition.
    pub fn ime_preedit(&self, text: &str, cursor: Option<(usize, usize)>) {
        if text.is_empty() {
            self.call(HostCall::ImeCancel);
        } else {
            self.call(HostCall::ImePreedit {
                selection: typing::preedit_selection(text, cursor),
                text: text.to_owned(),
            });
        }
    }

    /// Types `text` into the focused field, in the composition's place when
    /// there is one.
    pub fn ime_commit(&self, text: &str) {
        self.call(HostCall::ImeCommit(text.to_owned()));
    }

    /// Ends the composition without typing it.
    pub fn ime_cancel(&self) {
        self.call(HostCall::ImeCancel);
    }

    /// Takes the events waiting, oldest first. Returns at once.
    pub fn events(&self) -> OffscreenEvents {
        self.shared.events.drain()
    }

    /// Makes `call` on CEF's UI thread. Posted there from the UI thread too,
    /// since a call inside a frame callback must not re-enter CEF's paint.
    fn call(&self, call: HostCall) {
        let app = &self.browser.app;
        if app.cef_is_down() {
            return;
        }
        let mut task = HostTask::new(app.clone(), self.browser.id, self.key, call);
        if post_task(ThreadId::UI, &mut task) == 0 {
            debug!(
                "CEF refused a call for offscreen browser {}",
                self.browser.id.as_u32()
            );
        }
    }
}

impl Drop for OffscreenBrowser {
    fn drop(&mut self) {
        self.call(HostCall::Close { force: true });
    }
}

// By hand: the shared state has no Debug
impl fmt::Debug for OffscreenBrowser {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OffscreenBrowser")
            .field("id", &self.browser.id)
            .finish_non_exhaustive()
    }
}

/// Call on an offscreen browser's host.
#[derive(Clone, Debug)]
enum HostCall {
    Resized,
    ScreenChanged,
    Hidden(bool),
    FrameRate(c_int),
    BeginFrame,
    Repaint,
    Close {
        force: bool,
    },
    PointerMove {
        event: PointerEvent,
        leave: bool,
    },
    PointerButton {
        event: PointerEvent,
        button: PointerButton,
        up: bool,
        count: c_int,
    },
    PointerCancel(PointerEvent),
    Wheel {
        event: PointerEvent,
        x: c_int,
        y: c_int,
    },
    Focus(bool),
    Key(KeyEvent),
    ImePreedit {
        text: String,
        selection: Range,
    },
    ImeCommit(String),
    ImeCancel,
}

// Runs a call on the host of an offscreen browser still open
wrap_task! {
    struct HostTask {
        app: AppHandle,
        browser: BrowserId,
        key: PainterKey,
        call: HostCall,
    }

    impl Task {
        fn execute(&self) {
            if self.app.cef_is_down() {
                return;
            }
            let browser = self.app.registry().browsers.get(self.browser).map(|entry| entry.browser.clone());
            let Some(host) = browser.and_then(|browser| browser.host()) else {
                return;
            };
            match &self.call {
                HostCall::Resized => host.was_resized(),
                HostCall::ScreenChanged => host.notify_screen_info_changed(),
                HostCall::Hidden(hidden) => {
                    host.was_hidden((*hidden).into());
                    // A show past the compositor's saved frames evicts its new
                    // surface; a screen change gives it another
                    if !hidden {
                        host.notify_screen_info_changed();
                    }
                }
                HostCall::FrameRate(rate) => host.set_windowless_frame_rate(*rate),
                HostCall::BeginFrame => host.send_external_begin_frame(),
                HostCall::Repaint => host.invalidate(PaintElementType::VIEW),
                HostCall::Close { force } => host.close_browser((*force).into()),
                // A drag the page started takes the pointer while it lasts
                HostCall::PointerMove { event, leave } => {
                    let event = mouse_event(*event);
                    if !drag::moved(self.key, &host, &event, *leave) {
                        host.send_mouse_move_event(&event, (*leave).into());
                    }
                }
                HostCall::PointerButton { event, up: true, .. }
                    if drag::released(self.key, &host, &mouse_event(*event)) => {}
                HostCall::PointerButton { event, button, up, count } => {
                    host.send_mouse_click_event(&mouse_event(*event), button.cef(), (*up).into(), *count);
                }
                HostCall::PointerCancel(event) => {
                    drag::cancel(self.key, &host, &mouse_event(*event));
                    host.send_capture_lost_event();
                }
                HostCall::Wheel { event, x, y } => host.send_mouse_wheel_event(&mouse_event(*event), *x, *y),
                HostCall::Focus(focused) => host.set_focus((*focused).into()),
                HostCall::Key(event) => host.send_key_event(event),
                HostCall::ImePreedit { text, selection } => {
                    let underline = typing::preedit_underline(text);
                    host.ime_set_composition(Some(&CefString::from(text.as_str())), Some(&[underline]), &NO_RANGE, selection);
                }
                HostCall::ImeCommit(text) => host.ime_commit_text(Some(&CefString::from(text.as_str())), &NO_RANGE, 0),
                HostCall::ImeCancel => host.ime_cancel_composition(),
            }
        }
    }
}

/// Returns CEF's mouse event of `event`.
pub(super) fn mouse_event(event: PointerEvent) -> MouseEvent {
    MouseEvent {
        x: event.x,
        y: event.y,
        modifiers: event.modifiers,
    }
}

/// Returns CEF's whole frame rate for `hz`, within what the capturer takes.
fn frame_rate(hz: f64) -> c_int {
    hz.ceil().clamp(1.0, f64::from(MAX_FRAME_RATE)) as c_int
}

/// Creates an offscreen browser showing `url`; see
/// [`AppInstance::create_offscreen_browser`](crate::AppInstance::create_offscreen_browser).
pub(crate) fn create(
    app: &AppHandle,
    url: &str,
    options: OffscreenOptions,
) -> Result<OffscreenBrowser, RuntimeError> {
    if !app.offscreen_enabled() {
        return Err(RuntimeError::OffscreenNotEnabled);
    }
    if let Some(problem) = options.problem() {
        return Err(RuntimeError::InvalidOffscreenOptions(problem));
    }
    if app.is_ending() {
        return Err(RuntimeError::ShuttingDown);
    }
    let OffscreenOptions {
        width,
        height,
        scale,
        pacing,
        transparent,
        pixels,
        pixel_layers,
        texture,
        wake,
    } = options;
    let shared_texture = texture.is_some();

    let view = View::new(width, height, scale);
    let shared = Arc::new(Shared {
        view: Mutex::new(view),
        pointer: Mutex::new(Pointer::new(DoubleClick::of_os())),
        pacing,
        events: EventQueue::new(wake),
    });
    let key = paint::keep(Painter::new(pixels, pixel_layers, texture));
    let offscreen = OffscreenClient {
        key,
        shared: Arc::clone(&shared),
        render: paint::render_handler(key, Arc::clone(&shared)),
    };

    let info = WindowInfo {
        external_begin_frame_enabled: (pacing == Pacing::Host).into(),
        shared_texture_enabled: shared_texture.into(),
        ..WindowInfo::default().set_as_windowless(None)
    };
    let settings = BrowserSettings {
        windowless_frame_rate: match pacing {
            Pacing::Display { hz } => frame_rate(hz),
            Pacing::Host => HOST_FRAME_RATE,
        },
        background_color: if transparent { TRANSPARENT } else { WHITE },
        ..Default::default()
    };
    let mut client = KuroganeClient::new(
        app.clone(),
        BrowserType::Main,
        None,
        Surface::Offscreen(offscreen),
    );
    let Some(browser) = browser_host_create_browser_sync(
        &info,
        Some(&mut client),
        Some(&CefString::from(url)),
        &settings,
        None,
        None,
    ) else {
        paint::drop_painter(key);
        return Err(RuntimeError::BrowserCreationFailed);
    };
    debug!("create_offscreen_browser cef_id={}", browser.identifier());

    // CreateBrowserSync delivers on_after_created, which registers the
    // browser, before it returns; the guard ends with the statement
    let id = app
        .registry()
        .browsers
        .find_id_by_cef_id(browser.identifier());
    let Some(id) = id else {
        if let Some(host) = browser.host() {
            host.close_browser(1);
        }
        return Err(RuntimeError::BrowserCreationFailed);
    };

    let offscreen = OffscreenBrowser {
        browser: BrowserHandle {
            id,
            app: app.clone(),
        },
        shared,
        key,
    };
    if !view.shown() {
        offscreen.call(HostCall::Hidden(true));
    }
    Ok(offscreen)
}

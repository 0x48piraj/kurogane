//! Native window delegate.
//!
//! Controls how the native window behaves and embeds the
//! browser view into the platform window.

use tetsu::*;
use std::borrow::Cow;
use std::collections::VecDeque;

use tracing::{debug, warn};
use crate::browser_registry::{BrowserId, BrowserType};
use crate::cef_string::owned_by_cef;
use crate::client::KuroganeClient;
use crate::error::RuntimeError;
use crate::runtime::AppHandle;
use crate::window_options::{WindowOptions, WindowState};
use crate::window_registry::{WindowId, WindowKind};

/// Size and position requested by a page for a popup window.
///
/// A position is kept only when the page specifies both coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PopupGeometry {
    width: i32,
    height: i32,
    origin: Option<(i32, i32)>,
}

impl PopupGeometry {
    /// Returns the geometry requested by `features`, when a size was given.
    ///
    /// CEF sets `width_set` and `height_set` only when those values were
    /// specified by the page.
    pub(crate) fn requested(features: &PopupFeatures) -> Option<Self> {
        if features.width_set == 0 || features.height_set == 0 {
            return None;
        }
        let origin =
            (features.x_set != 0 && features.y_set != 0).then_some((features.x, features.y));
        Some(Self {
            width: features.width,
            height: features.height,
            origin,
        })
    }

    fn bounds(self) -> Option<Rect> {
        let (x, y) = self.origin?;
        Some(Rect {
            x,
            y,
            width: self.width,
            height: self.height,
        })
    }

    fn size(self) -> Size {
        Size {
            width: self.width,
            height: self.height,
        }
    }
}

/// Popups CEF has allowed but not shown yet, in creation order.
///
/// Each entry keeps the popup's ID and the size and position requested by
/// its page. Pending entries are removed when the popup is shown, aborted, or
/// its opener closes.
#[derive(Debug, Default)]
pub(crate) struct PendingPopups(VecDeque<(i32, Option<PopupGeometry>)>);

impl PendingPopups {
    /// Records a pending popup and its requested geometry.
    pub(crate) fn push(&mut self, popup_id: i32, requested: Option<PopupGeometry>) {
        self.0.push_back((popup_id, requested));
    }

    /// Removes the popup being shown and returns its requested geometry.
    pub(crate) fn take(&mut self) -> Option<PopupGeometry> {
        self.0.pop_front().and_then(|(_, requested)| requested)
    }

    /// Removes a popup CEF gave up on before showing it.
    pub(crate) fn abort(&mut self, popup_id: i32) {
        self.0.retain(|&(id, _)| id != popup_id);
    }
}

/// What the system shows of every window Kurogane opens, whatever opened
/// it: the application's identity, plain data given at startup.
#[derive(Clone, Debug, Default)]
pub(crate) struct WindowIdentity {
    /// The Linux window class: WM_CLASS under X11, the app_id under Wayland
    pub class: Option<String>,
    /// The windows' icon, a PNG, decoded for each window as it is created
    pub icon: Option<Cow<'static, [u8]>>,
}

impl WindowIdentity {
    /// Returns the identity with the executable's file name as its class
    /// when the application named none.
    pub fn or_executable_class(mut self) -> Self {
        if self.class.is_none() {
            self.class = executable_class();
        }
        self
    }
}

/// Returns the running executable's file name when it can be a window class.
fn executable_class() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let name = exe.file_name()?.to_str()?;
    (!name.chars().any(char::is_control)).then(|| name.to_owned())
}

/// The size, in DIP, CEF takes a window icon at: it refuses any other
/// (CefWindowView::SetWindowIcon), and the app icon at any size.
const WINDOW_ICON_SIDE: f32 = 16.0;

/// A window's two icons from `png`, as CEF images: the window icon (the
/// title bar), the PNG's pixels at the scale that makes it 16 DIP, and the
/// app icon (the taskbar, the window switcher) as it is. Made with each
/// window, on the UI thread: a CEF image may live only between
/// CefInitialize and CefShutdown, which one kept in the runtime's services
/// would outlive.
fn window_icons(png: &[u8]) -> Option<(Image, Image)> {
    let app = image_create()?;
    if app.add_png(1.0, Some(png)) == 0 {
        warn!("the window icon is not a PNG CEF can read; the window keeps its default icon");
        return None;
    }
    let side = app.width().max(app.height()) as f32;
    let window = image_create()?;
    window.add_png(side / WINDOW_ICON_SIDE, Some(png));
    Some((window, app))
}

/// Where a new top-level window opens and how it first shows.
#[derive(Clone, Debug)]
pub(crate) enum Opening {
    /// A window of the application's own, as its options say: the start
    /// window, one from create_window, or one a page opened as the
    /// application's.
    Application(WindowOptions),
    /// A popup's window, at the size and position its page asked for;
    /// without one CEF gives the popup its default 800x600 window.
    Popup(Option<PopupGeometry>),
}

impl Opening {
    /// The window's size when its bounds are empty; empty lets CEF choose.
    fn preferred_size(&self) -> Size {
        match self {
            // Always given initial bounds
            Self::Application(_) => Size::default(),
            Self::Popup(requested) => requested.map(PopupGeometry::size).unwrap_or_default(),
        }
    }

    /// Where the window opens, as the application or the page asked
    /// (an application's placement is then brought onto a display, see
    /// [`on_a_display`]). Empty when nobody placed it: an application window
    /// is then centred at its size ([`centred`]), and CEF gives a popup the
    /// size from preferred_size at the origin (0,0) (cef_window_delegate.h,
    /// GetInitialBounds).
    fn initial_bounds(&self) -> Rect {
        match self {
            Self::Application(options) => options.requested_bounds().unwrap_or_default(),
            Self::Popup(requested) => requested
                .and_then(PopupGeometry::bounds)
                .unwrap_or_default(),
        }
    }

    /// How the window first shows; a popup shows normally.
    fn show_state(&self) -> ShowState {
        match self {
            Self::Application(options) => options.initial_state().into(),
            Self::Popup(_) => ShowState::NORMAL,
        }
    }

    /// The state the window shows in once restored, before it has shown in
    /// any: opened minimized or hidden, it comes back normal.
    fn restored_state(&self) -> WindowState {
        match self {
            Self::Application(options) => options.initial_state().restored(),
            Self::Popup(_) => WindowState::Normal,
        }
    }

    /// The size the user cannot make the window smaller than; none for a
    /// popup.
    fn minimum_size(&self) -> Size {
        match self {
            Self::Application(options) => options
                .minimum()
                .map(|(width, height)| Size { width, height })
                .unwrap_or_default(),
            Self::Popup(_) => Size::default(),
        }
    }

    /// Whether the window takes its page's title, as a browser tab does: a
    /// popup's always does, an application window's unless its options fix
    /// one.
    fn follows_title(&self) -> bool {
        match self {
            Self::Application(options) => options.fixed_title().is_none(),
            Self::Popup(_) => true,
        }
    }

    fn kind(&self) -> WindowKind {
        match self {
            Self::Application(_) => WindowKind::Application,
            Self::Popup(_) => WindowKind::Popup,
        }
    }

    /// The name the application gave the window; a popup has none.
    fn name(&self) -> Option<&str> {
        match self {
            Self::Application(options) => options.window_name(),
            Self::Popup(_) => None,
        }
    }
}

/// The least of a window, in DIP each way, that must show on a display for
/// it to stay where it was put: Chromium's rule for its own windows
/// (chrome/browser/ui/window_sizer, kMinVisibleWidth and kMinVisibleHeight).
const MIN_VISIBLE: i32 = 30;

/// An application window's size when its options give none, in DIP: the
/// size CEF gives a window it is asked to size itself.
const DEFAULT_SIZE: (i32, i32) = (800, 600);

/// Whether the frame around a window's content is known as CEF creates the
/// window, before it shows. Under X11 the window manager adds the frame once
/// the window maps.
const FRAME_KNOWN_AT_CREATION: bool = !cfg!(target_os = "linux");

/// The frame the system draws around a window's content, in DIP on each
/// side. Zero for a window the window manager has not framed yet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Frame {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

impl Frame {
    /// Returns the frame around `window`'s content.
    fn of(window: &Window) -> Self {
        Self::between(
            &window.bounds_in_screen(),
            &window.client_area_bounds_in_screen(),
        )
    }

    /// Returns the frame between a whole window `outer` and its `content`.
    fn between(outer: &Rect, content: &Rect) -> Self {
        Self {
            left: content.x - outer.x,
            top: content.y - outer.y,
            right: outer.x + outer.width - content.x - content.width,
            bottom: outer.y + outer.height - content.y - content.height,
        }
    }

    /// Returns the whole window around `content`.
    fn around(&self, content: &Rect) -> Rect {
        Rect {
            x: content.x - self.left,
            y: content.y - self.top,
            width: content.width + self.left + self.right,
            height: content.height + self.top + self.bottom,
        }
    }

    /// Returns the content inside the whole window `outer`.
    fn within(&self, outer: &Rect) -> Rect {
        Rect {
            x: outer.x + self.left,
            y: outer.y + self.top,
            width: outer.width - self.left - self.right,
            height: outer.height - self.top - self.bottom,
        }
    }
}

/// Where an application window opens as a whole, its frame known: around
/// its placement and brought onto a display, or centred at its size with
/// the frame. UI thread.
fn framed_opening_bounds(options: &WindowOptions, frame: &Frame) -> Rect {
    match options.requested_bounds() {
        Some(content) => on_a_display(frame.around(&content)),
        None => {
            let (width, height) = options.requested_size().unwrap_or(DEFAULT_SIZE);
            centred(
                width + frame.left + frame.right,
                height + frame.top + frame.bottom,
            )
        }
    }
}

/// Shows a window CEF opened normal in `state`. Maximized or minimized it
/// shows as it takes the state, and hidden it stays unshown. UI thread.
fn show_in(window: &Window, state: WindowState) {
    match state {
        WindowState::Normal => window.show(),
        WindowState::Maximized => window.maximize(),
        WindowState::Minimized => window.minimize(),
        WindowState::Fullscreen => {
            window.set_fullscreen(1);
            window.show();
        }
        WindowState::Hidden => {}
    }
}

/// Where a window opens: an application window's content where its
/// placement says, brought onto a display, or centred at its size; a popup
/// where its page asked, empty for CEF to choose. UI thread.
fn opening_bounds(opening: &Opening) -> Rect {
    match opening {
        Opening::Application(options) => match options.requested_bounds() {
            Some(_) => on_a_display(opening.initial_bounds()),
            None => {
                let (width, height) = options.requested_size().unwrap_or(DEFAULT_SIZE);
                centred(width, height)
            }
        },
        Opening::Popup(_) => opening.initial_bounds(),
    }
}

/// A window of `width` by `height` centred on the primary display's work
/// area, made to fit it. Computed before the window exists, so a window
/// that opens maximized, minimized or fullscreen restores to it. UI thread.
fn centred(width: i32, height: i32) -> Rect {
    match display_get_primary() {
        Some(display) => centre(width, height, &display.work_area()),
        None => Rect {
            x: 0,
            y: 0,
            width,
            height,
        },
    }
}

/// A window of `width` by `height` centred on the work area `area`, made to
/// fit it.
fn centre(width: i32, height: i32, area: &Rect) -> Rect {
    let x = area.x + (area.width - width) / 2;
    let y = area.y + (area.height - height) / 2;
    fit(
        Rect {
            x,
            y,
            width,
            height,
        },
        area,
    )
}

/// `bounds` on the display that shows most of them, or the nearest one when
/// none does: made to fit its work area (and then inside it), the top edge
/// not above it, and moved inside it when less than [`MIN_VISIBLE`] of the
/// window shows there. UI thread.
fn on_a_display(bounds: Rect) -> Rect {
    match display_get_matching_bounds(Some(&bounds), 0) {
        Some(display) => fit(bounds, &display.work_area()),
        None => bounds,
    }
}

/// `bounds` made to show on the work area `area` (see [`on_a_display`]).
fn fit(bounds: Rect, area: &Rect) -> Rect {
    let width = bounds.width.min(area.width);
    let height = bounds.height.min(area.height);
    let mut x = bounds.x;
    let mut y = bounds.y.max(area.y);
    let shown_x = (x + width).min(area.x + area.width) - x.max(area.x);
    let shown_y = (y + height).min(area.y + area.height) - y.max(area.y);
    let lost = shown_x < MIN_VISIBLE.min(width) || shown_y < MIN_VISIBLE.min(height);
    if lost || width < bounds.width {
        x = x.clamp(area.x, area.x + area.width - width);
    }
    if lost || height < bounds.height {
        y = y.clamp(area.y, area.y + area.height - height);
    }
    Rect {
        x,
        y,
        width,
        height,
    }
}

wrap_window_delegate! {
    pub struct KuroganeWindowDelegate {
        window_id: WindowId,
        browser_view: BrowserView,
        app: AppHandle,
        // Known for a popup, whose browser exists before its window
        // (cef_browser_view_delegate.h:95-97). A main window's browser is
        // created when the window adds its view, and on_browser_created
        // links it
        browser_id: Option<BrowserId>,
        opening: Opening,
    }

    impl ViewDelegate {
        fn preferred_size(&self, _view: Option<&mut View>) -> Size {
            self.opening.preferred_size()
        }

        fn minimum_size(&self, _view: Option<&mut View>) -> Size {
            self.opening.minimum_size()
        }
    }

    impl PanelDelegate {}

    impl WindowDelegate {
        fn initial_bounds(&self, _window: Option<&mut Window>) -> Rect {
            opening_bounds(&self.opening)
        }

        // A window CEF opens in another state reports that state's bounds as
        // it is created, so where the frame is known an application window
        // opens normal and on_window_created applies its state
        fn initial_show_state(&self, _window: Option<&mut Window>) -> ShowState {
            match self.opening {
                Opening::Application(_) if FRAME_KNOWN_AT_CREATION => ShowState::NORMAL,
                _ => self.opening.show_state(),
            }
        }

        // Linux only. Sets the class the window manager knows the window by,
        // the application's or the executable's name. The strings are CEF's
        // own, which cef-rs's write of this struct back to CEF keeps
        // (crate::cef_string)
        fn linux_window_properties(
            &self,
            _window: Option<&mut Window>,
            properties: Option<&mut LinuxWindowProperties>,
        ) -> ::std::os::raw::c_int {
            let (Some(class), Some(properties)) = (&self.app.window_identity().class, properties) else {
                return 0;
            };
            properties.wayland_app_id = owned_by_cef(class);
            properties.wm_class_class = owned_by_cef(class);
            properties.wm_class_name = owned_by_cef(class);
            debug!("[Window] {} has the class {class}", self.window_id.as_u32());
            1
        }

        fn on_window_created(&self, window: Option<&mut Window>) {
            let Some(window) = window else {
                return;
            };
            // Registered before the view is added, which creates a main
            // window's browser; the guard ends with the statement
            self.app.registry().windows.insert(
                self.window_id,
                window.clone(),
                self.browser_id,
                self.opening.follows_title(),
                self.opening.kind(),
                self.opening.restored_state(),
            );
            // An application window's options place its content. Where the
            // frame is known the whole window goes around that content
            // before it shows; under X11 the content is the window's bounds.
            // A popup restores to where CEF put it. The guard ends with the
            // statement, after CEF answered
            let initial = match &self.opening {
                Opening::Application(options) if FRAME_KNOWN_AT_CREATION => {
                    let frame = Frame::of(window);
                    let outer = framed_opening_bounds(options, &frame);
                    window.set_bounds(Some(&outer));
                    frame.within(&outer)
                }
                Opening::Application(_) => opening_bounds(&self.opening),
                Opening::Popup(_) => window.client_area_bounds_in_screen(),
            };
            self.app.registry().windows.set_restored(self.window_id, initial);

            window.add_child_view(Some(&mut (&self.browser_view).into()));
            if let Opening::Application(options) = &self.opening
                && let Some(title) = options.fixed_title()
            {
                window.set_title(Some(&CefString::from(title)));
            }
            if let Some(png) = &self.app.window_identity().icon
                && let Some((mut small, mut large)) = window_icons(png)
            {
                window.set_window_icon(Some(&mut small));
                window.set_window_app_icon(Some(&mut large));
            }
            match &self.opening {
                Opening::Application(options) if FRAME_KNOWN_AT_CREATION => {
                    show_in(window, options.initial_state());
                }
                _ if self.opening.show_state() != ShowState::HIDDEN => window.show(),
                _ => {}
            }
            match &self.opening {
                Opening::Application(options) => debug!(
                    "[Window] {} opened at {:?}, its content at {:?}{}",
                    self.window_id.as_u32(),
                    window.bounds_in_screen(),
                    window.client_area_bounds_in_screen(),
                    options
                        .window_name()
                        .map(|name| format!(", named {name}"))
                        .unwrap_or_default()
                ),
                Opening::Popup(_) => debug!("Popup window shown at {:?}", window.bounds()),
            }
        }

        fn on_window_bounds_changed(&self, window: Option<&mut Window>, new_bounds: Option<&Rect>) {
            let (Some(window), Some(bounds)) = (window, new_bounds) else {
                return;
            };
            // CEF reports the new state with the bounds (measured on
            // Windows): a maximized, minimized or fullscreen window's bounds
            // are not where it restores to
            let (maximized, minimized, fullscreen) =
                (window.is_maximized(), window.is_minimized(), window.is_fullscreen());
            let content = window.client_area_bounds_in_screen();
            debug!(
                "[Window] {} bounds {},{} {}x{} content {},{} {}x{} maximized={maximized} minimized={minimized} fullscreen={fullscreen}",
                self.window_id.as_u32(),
                bounds.x,
                bounds.y,
                bounds.width,
                bounds.height,
                content.x,
                content.y,
                content.width,
                content.height,
            );
            // A minimized window comes back as it last showed, so only a
            // window not minimized says how that is
            if minimized == 0 {
                let state = if fullscreen != 0 {
                    WindowState::Fullscreen
                } else if maximized != 0 {
                    WindowState::Maximized
                } else {
                    WindowState::Normal
                };
                // The guard ends with the statement
                self.app.registry().windows.shown(self.window_id, state, &content);
            }
        }

        fn on_window_destroyed(&self, _window: Option<&mut Window>) {
            debug!("[Window] {} destroyed", self.window_id.as_u32());
            self.app.registry().windows.unregister(self.window_id);
        }

        // cef-rs answers 0 for a callback left out, where CEF's C++ defaults
        // answer true; these restore them
        fn with_standard_window_buttons(
            &self,
            _window: Option<&mut Window>,
        ) -> ::std::os::raw::c_int {
            1
        }

        fn can_resize(&self, _window: Option<&mut Window>) -> ::std::os::raw::c_int {
            1
        }

        fn can_maximize(&self, _window: Option<&mut Window>) -> ::std::os::raw::c_int {
            1
        }

        fn can_minimize(&self, _window: Option<&mut Window>) -> ::std::os::raw::c_int {
            1
        }

        // CEF's default is true; the window asks its browser instead, as
        // cefsimple's CanClose does, so the page's unload handlers can run
        fn can_close(&self, _window: Option<&mut Window>) -> ::std::os::raw::c_int {
            if let Some(browser) = self.browser_view.browser() && let Some(host) = browser.host() {
                let answer = host.try_close_browser();
                debug!("[Window] {} can close: {answer}", self.window_id.as_u32());
                return answer;
            }
            debug!("[Window] {} can close: no browser", self.window_id.as_u32());
            1
        }
    }
}

wrap_browser_view_delegate! {
    pub struct KuroganeBrowserViewDelegate {
        app: AppHandle,
        // The window this delegate's BrowserView is shown in; none for a
        // popup's view, whose window its opener's delegate makes
        window_id: Option<WindowId>,
    }

    impl ViewDelegate {}

    impl BrowserViewDelegate {
        fn on_browser_created(
            &self,
            _browser_view: Option<&mut BrowserView>,
            browser: Option<&mut Browser>,
        ) {
            // A popup is registered and linked to its window in its opener's
            // on_popup_browser_view_created
            let (Some(browser), Some(window_id)) = (browser, self.window_id) else {
                return;
            };

            let browser_id = self
                .app
                .registry()
                .browsers
                .ensure_registered(browser, BrowserType::Main, None);

            if self.app.registry().windows.link(window_id, browser_id) {
                debug!(
                    "[BrowserRegistry] linked browser {} to window {}",
                    browser_id.as_u32(),
                    window_id.as_u32()
                );
            }
        }

        // cef-rs answers None here where CEF's C++ default answers this
        // delegate. Without one, a popup's own popups and DevTools opened for
        // it get CEF's window, which the runtime neither tracks nor sizes
        fn delegate_for_popup_browser_view(
            &self,
            _browser_view: Option<&mut BrowserView>,
            _settings: Option<&BrowserSettings>,
            _client: Option<&mut Client>,
            _is_devtools: ::std::os::raw::c_int,
        ) -> Option<BrowserViewDelegate> {
            Some(KuroganeBrowserViewDelegate::new(self.app.clone(), None))
        }

        fn on_popup_browser_view_created(
            &self,
            browser_view: Option<&mut BrowserView>,
            popup_browser_view: Option<&mut BrowserView>,
            is_devtools: ::std::os::raw::c_int,
        ) -> ::std::os::raw::c_int {
            debug!("[BrowserViewDelegate] popup browser view created");

            if let Some(pbv) = popup_browser_view {
                // Derive parent/opener BrowserId from the parent BrowserView
                let parent_id = browser_view.and_then(|bv| bv.browser())
                    .and_then(|b| {
                        let reg = self.app.registry();
                        reg.browsers.find_id_by_browser(&b)
                    });

                // Classified here, where its kind and opener are known,
                // whether or not on_after_created registered it first
                let browser_type = if is_devtools != 0 { BrowserType::DevTools } else { BrowserType::Popup };
                let browser_id = pbv.browser().map(|browser| {
                    let mut reg = self.app.registry();
                    let id = reg.browsers.ensure_registered(&browser, browser_type, parent_id);
                    reg.browsers.classify(id, browser_type, parent_id);
                    debug!("[BrowserViewDelegate] registered popup browser");
                    id
                });

                // What the page asked of the window, kept by the opener since
                // on_before_popup. DevTools popups come from
                // on_before_dev_tools_popup and are not in its list
                let requested = match parent_id {
                    Some(opener) if is_devtools == 0 => self
                        .app
                        .registry()
                        .browsers
                        .get_mut(opener)
                        .and_then(|state| state.pending_popups.take()),
                    _ => None,
                };
                debug!("[BrowserViewDelegate] popup window requested {:?}", requested);

                // CEF made the popup's view; the window adds it and shows
                // itself, as cefsimple's popup window does
                let window_id = {
                    let mut reg = self.app.registry();
                    reg.windows.allocate_id()
                };

                let mut delegate = KuroganeWindowDelegate::new(
                    window_id,
                    pbv.clone(),
                    self.app.clone(),
                    browser_id,
                    Opening::Popup(requested),
                );
                if window_create_top_level(Some(&mut delegate)).is_some() {
                    debug!("[BrowserViewDelegate] popup window created");
                    return 1;
                }
            }

            0
        }
    }
}

/// Opens `url` in a new browser, in a new top-level window as `opening` says.
/// UI thread, where CEF creates browsers and windows.
///
/// The browser is created when the window adds its view
/// (cef_browser_view.h:53-54), and is registered and linked to the window
/// then. A window named holds its name from here on, until its browser's
/// close is reported; refused when an open window holds it.
pub(crate) fn open_browser_window(
    app: &AppHandle,
    url: &str,
    opening: Opening,
) -> Result<WindowId, RuntimeError> {
    if app.is_ending() {
        return Err(RuntimeError::ShuttingDown);
    }
    // The guard ends with the block, before any CEF call
    let window_id = {
        let mut reg = app.registry();
        match opening.name() {
            Some(name) => reg.windows.allocate_named(name).map_err(|holder| {
                RuntimeError::WindowNameTaken {
                    name: name.to_owned(),
                    window: holder,
                }
            })?,
            None => reg.windows.allocate_id(),
        }
    };
    let opened = create_browser_window(app, url, window_id, opening);
    if opened.is_err() {
        // No window will hold it; the guard ends with the statement
        app.registry().windows.release_name(window_id);
    }
    opened
}

/// The browser and the top-level window of [`open_browser_window`], in
/// window `window_id`.
fn create_browser_window(
    app: &AppHandle,
    url: &str,
    window_id: WindowId,
    opening: Opening,
) -> Result<WindowId, RuntimeError> {
    let mut client = KuroganeClient::new(app.clone(), BrowserType::Main, None);
    let mut view_delegate = KuroganeBrowserViewDelegate::new(app.clone(), Some(window_id));

    debug!(
        "Creating a browser for {url} in window {}",
        window_id.as_u32()
    );
    // Chromium's status bubble shows the URL of a hovered link in the
    // window's corner, a browser's affordance and, for an application served
    // from localhost, its address with whatever the query carries. Popups
    // inherit their opener's settings
    let settings = BrowserSettings {
        chrome_status_bubble: State::DISABLED,
        ..Default::default()
    };
    let browser_view = browser_view_create(
        Some(&mut client),
        Some(&CefString::from(url)),
        Some(&settings),
        None,
        None,
        Some(&mut view_delegate),
    )
    .ok_or(RuntimeError::BrowserCreationFailed)?;

    let mut delegate =
        KuroganeWindowDelegate::new(window_id, browser_view, app.clone(), None, opening);
    window_create_top_level(Some(&mut delegate)).ok_or(RuntimeError::WindowCreationFailed)?;
    debug!("Top-level window created");

    Ok(window_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn features(
        x: Option<i32>,
        y: Option<i32>,
        width: Option<i32>,
        height: Option<i32>,
    ) -> PopupFeatures {
        PopupFeatures {
            x: x.unwrap_or_default(),
            x_set: x.is_some().into(),
            y: y.unwrap_or_default(),
            y_set: y.is_some().into(),
            width: width.unwrap_or_default(),
            width_set: width.is_some().into(),
            height: height.unwrap_or_default(),
            height_set: height.is_some().into(),
            ..Default::default()
        }
    }

    fn size_only(width: i32, height: i32) -> PopupGeometry {
        PopupGeometry {
            width,
            height,
            origin: None,
        }
    }

    #[test]
    fn a_size_counts_only_whole() {
        let asked = |f| PopupGeometry::requested(&f);
        assert_eq!(asked(features(None, None, None, None)), None);
        assert_eq!(asked(features(Some(10), Some(20), Some(320), None)), None);
        assert_eq!(
            asked(features(None, None, Some(320), Some(200))),
            Some(size_only(320, 200))
        );
    }

    #[test]
    fn a_position_counts_only_whole() {
        let asked = |f| PopupGeometry::requested(&f).unwrap();
        assert_eq!(
            asked(features(Some(10), None, Some(320), Some(200))).origin,
            None
        );
        assert_eq!(
            asked(features(Some(10), Some(20), Some(320), Some(200))).origin,
            Some((10, 20))
        );
    }

    #[test]
    fn only_a_placed_popup_has_initial_bounds() {
        assert!(size_only(320, 200).bounds().is_none());

        let placed = PopupGeometry {
            origin: Some((10, 20)),
            ..size_only(320, 200)
        };
        let Rect {
            x,
            y,
            width,
            height,
        } = placed.bounds().unwrap();
        assert_eq!((x, y, width, height), (10, 20, 320, 200));
    }

    #[test]
    fn popups_are_shown_in_the_order_they_were_opened() {
        let mut pending = PendingPopups::default();
        pending.push(1, Some(size_only(320, 200)));
        pending.push(2, Some(size_only(640, 480)));

        assert_eq!(pending.take(), Some(size_only(320, 200)));
        assert_eq!(pending.take(), Some(size_only(640, 480)));
        assert_eq!(pending.take(), None);
    }

    #[test]
    fn a_popup_that_asked_for_nothing_keeps_its_turn() {
        let mut pending = PendingPopups::default();
        pending.push(1, None);
        pending.push(2, Some(size_only(320, 200)));

        assert_eq!(pending.take(), None);
        assert_eq!(pending.take(), Some(size_only(320, 200)));
    }

    #[test]
    fn an_aborted_popup_leaves_the_others_in_order() {
        let mut pending = PendingPopups::default();
        pending.push(1, Some(size_only(100, 100)));
        pending.push(2, Some(size_only(200, 200)));
        pending.push(3, Some(size_only(300, 300)));
        pending.abort(2);

        assert_eq!(pending.take(), Some(size_only(100, 100)));
        assert_eq!(pending.take(), Some(size_only(300, 300)));
    }

    fn application(options: WindowOptions) -> Opening {
        Opening::Application(options)
    }

    fn rect(x: i32, y: i32, width: i32, height: i32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    fn parts(r: Rect) -> (i32, i32, i32, i32) {
        (r.x, r.y, r.width, r.height)
    }

    #[test]
    fn an_application_window_opens_at_its_placement_in_its_state() {
        let opening = application(WindowOptions::new().placement(crate::WindowPlacement {
            x: 10,
            y: 20,
            width: 800,
            height: 600,
            state: WindowState::Hidden,
        }));
        assert_eq!(parts(opening.initial_bounds()), (10, 20, 800, 600));
        // Sized by its placement, or centred at its size once created
        let Size { width, height } = opening.preferred_size();
        assert_eq!((width, height), (0, 0));
        assert_eq!(opening.show_state(), ShowState::HIDDEN);
        // Shown later, it shows normally
        assert_eq!(opening.restored_state(), WindowState::Normal);
    }

    #[test]
    fn only_an_application_window_has_a_name() {
        assert_eq!(
            application(WindowOptions::new().name("main")).name(),
            Some("main")
        );
        assert_eq!(application(WindowOptions::new()).name(), None);
        assert_eq!(Opening::Popup(None).name(), None);
    }

    #[test]
    fn an_application_window_without_bounds_is_left_to_be_centred() {
        for options in [WindowOptions::new(), WindowOptions::new().size(1100, 720)] {
            assert_eq!(parts(application(options).initial_bounds()), (0, 0, 0, 0));
        }
    }

    #[test]
    fn an_application_window_not_placed_is_centred_at_its_size() {
        let area = rect(0, 0, 1920, 1032);
        assert_eq!(parts(centre(900, 640, &area)), (510, 196, 900, 640));
        // A primary display that does not start at the origin
        assert_eq!(
            parts(centre(800, 600, &rect(-1920, 0, 1920, 1040))),
            (-1360, 220, 800, 600)
        );
        // Larger than the work area: made to fit it
        assert_eq!(parts(centre(3000, 2000, &area)), (0, 0, 1920, 1032));
    }

    #[test]
    fn a_frame_goes_around_the_content_and_comes_off_again() {
        // A Windows 11 frame at 100%, its invisible borders included
        let frame = Frame::between(&rect(502, 176, 916, 679), &rect(510, 207, 900, 640));
        assert_eq!(
            frame,
            Frame {
                left: 8,
                top: 31,
                right: 8,
                bottom: 8
            }
        );
        let content = rect(200, 150, 900, 600);
        assert_eq!(parts(frame.around(&content)), (192, 119, 916, 639));
        assert_eq!(
            parts(frame.within(&frame.around(&content))),
            (200, 150, 900, 600)
        );
        // No frame yet (X11 before the window maps)
        assert_eq!(Frame::between(&content, &content), Frame::default());
    }

    #[test]
    fn a_framed_window_is_centred_and_fitted_whole() {
        let area = rect(0, 0, 1920, 1032);
        // 900x640 of content in a 916x679 window
        assert_eq!(parts(centre(916, 679, &area)), (502, 176, 916, 679));
        // Too large for the work area: the whole window fits it
        assert_eq!(parts(centre(1916, 1071, &area)), (2, 0, 1916, 1032));
    }

    #[test]
    fn an_application_window_has_its_minimum_and_a_popup_none() {
        let Size { width, height } =
            application(WindowOptions::new().min_size(640, 480)).minimum_size();
        assert_eq!((width, height), (640, 480));
        let Size { width, height } = application(WindowOptions::new()).minimum_size();
        assert_eq!((width, height), (0, 0));
        let Size { width, height } = Opening::Popup(Some(size_only(320, 200))).minimum_size();
        assert_eq!((width, height), (0, 0));
    }

    #[test]
    fn a_window_takes_its_page_s_title_unless_its_options_fix_one() {
        assert!(application(WindowOptions::new()).follows_title());
        assert!(!application(WindowOptions::new().title("Notes")).follows_title());
        assert!(Opening::Popup(None).follows_title());
    }

    #[test]
    fn a_window_a_display_shows_stays_where_it_was_put() {
        let area = rect(0, 0, 1920, 1032);
        // Snapped to the left edge, its invisible border off the screen
        assert_eq!(
            parts(fit(rect(-7, 0, 974, 1039), &area)),
            (-7, 0, 974, 1032)
        );
        assert_eq!(
            parts(fit(rect(200, 150, 900, 600), &area)),
            (200, 150, 900, 600)
        );
        // Mostly off the screen, but 30 DIP of it still show each way
        assert_eq!(
            parts(fit(rect(1890, 1002, 900, 600), &area)),
            (1890, 1002, 900, 600)
        );
    }

    #[test]
    fn a_window_no_display_shows_is_brought_onto_one() {
        let area = rect(0, 0, 1920, 1032);
        // A display to the left that is gone
        assert_eq!(
            parts(fit(rect(-2500, 100, 900, 600), &area)),
            (0, 100, 900, 600)
        );
        // Less than 30 DIP showing
        assert_eq!(
            parts(fit(rect(1900, 100, 900, 600), &area)),
            (1020, 100, 900, 600)
        );
        assert_eq!(
            parts(fit(rect(100, 1010, 900, 600), &area)),
            (100, 432, 900, 600)
        );
        // The title bar never above the work area
        assert_eq!(
            parts(fit(rect(100, -300, 900, 600), &area)),
            (100, 0, 900, 600)
        );
        // Larger than the work area: made to fit it
        assert_eq!(
            parts(fit(rect(-50, -50, 3000, 2000), &area)),
            (0, 0, 1920, 1032)
        );
        // A work area that does not start at the origin
        let right = rect(1920, 0, 1920, 1032);
        assert_eq!(
            parts(fit(rect(5000, 200, 800, 600), &right)),
            (3040, 200, 800, 600)
        );
    }

    #[test]
    fn a_popup_opens_at_what_its_page_asked_for() {
        // A size alone: CEF places the window, at that size
        let sized = Opening::Popup(Some(size_only(320, 200)));
        let Rect { width, height, .. } = sized.initial_bounds();
        assert_eq!((width, height), (0, 0));
        let Size { width, height } = sized.preferred_size();
        assert_eq!((width, height), (320, 200));
        assert_eq!(sized.show_state(), ShowState::NORMAL);

        let placed = Opening::Popup(Some(PopupGeometry {
            origin: Some((10, 20)),
            ..size_only(320, 200)
        }));
        let Rect {
            x,
            y,
            width,
            height,
        } = placed.initial_bounds();
        assert_eq!((x, y, width, height), (10, 20, 320, 200));

        // Nothing asked for: CEF's default window
        let default = Opening::Popup(None);
        let Rect { width, height, .. } = default.initial_bounds();
        assert_eq!((width, height), (0, 0));
        let Size { width, height } = default.preferred_size();
        assert_eq!((width, height), (0, 0));
        assert_eq!(default.show_state(), ShowState::NORMAL);
    }
}

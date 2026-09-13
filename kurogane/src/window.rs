//! Native window delegate.
//!
//! Controls how the native window behaves and embeds the
//! browser view into the platform window.

use cef::*;
use std::collections::VecDeque;
use std::sync::Arc;

use crate::debug;
use crate::app::ClientAppBrowserDelegate;
use crate::browser_registry::{BrowserId, BrowserType};
use crate::client::KuroganeClient;
use crate::error::RuntimeError;
use crate::runtime::{AppHandle, BrowserBounds, WindowState};
use crate::window_registry::WindowId;

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

/// Where a new top-level window opens and how it first shows.
#[derive(Clone, Debug)]
pub(crate) enum Placement {
    /// A window of the application's own, at `bounds` (empty lets CEF choose)
    /// in `show_state`.
    Main { bounds: Rect, show_state: ShowState },
    /// A popup's window, at the size and position its page asked for;
    /// without one CEF gives the popup its default 800x600 window.
    Popup(Option<PopupGeometry>),
}

impl Placement {
    /// The window's size when its bounds are empty; empty lets CEF choose.
    fn preferred_size(&self) -> Size {
        match self {
            Self::Main { .. } => Size::default(),
            Self::Popup(requested) => requested.map(PopupGeometry::size).unwrap_or_default(),
        }
    }

    /// Where the window opens. Empty unless the application or the page
    /// placed it: CEF then takes the size from preferred_size
    /// (cef_window_delegate.h:129-134).
    fn initial_bounds(&self) -> Rect {
        match self {
            Self::Main { bounds, .. } => bounds.clone(),
            Self::Popup(requested) => requested
                .and_then(PopupGeometry::bounds)
                .unwrap_or_default(),
        }
    }

    /// How the window first shows; a popup shows normally.
    fn show_state(&self) -> ShowState {
        match self {
            Self::Main { show_state, .. } => *show_state,
            Self::Popup(_) => ShowState::NORMAL,
        }
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
        placement: Placement,
        // Told when the window closes; empty for every window but the one
        // Kurogane opens itself
        closing: Vec<Arc<dyn ClientAppBrowserDelegate>>,
    }

    impl ViewDelegate {
        fn preferred_size(&self, _view: Option<&mut View>) -> Size {
            self.placement.preferred_size()
        }
    }

    impl PanelDelegate {}

    impl WindowDelegate {
        fn initial_bounds(&self, _window: Option<&mut Window>) -> Rect {
            self.placement.initial_bounds()
        }

        fn initial_show_state(&self, _window: Option<&mut Window>) -> ShowState {
            self.placement.show_state()
        }

        fn on_window_created(&self, window: Option<&mut Window>) {
            let Some(window) = window else {
                return;
            };
            // Registered before the view is added, which creates a main
            // window's browser; the guard ends with the statement
            self.app
                .registry()
                .windows
                .insert(self.window_id, window.clone(), self.browser_id);

            window.add_child_view(Some(&mut (&self.browser_view).into()));
            if self.placement.show_state() != ShowState::HIDDEN {
                window.show();
            }
            match self.placement {
                Placement::Main { .. } => debug!("Window shown"),
                Placement::Popup(_) => debug!("Popup window shown at {:?}", window.bounds()),
            }
        }

        fn on_window_closing(&self, window: Option<&mut Window>) {
            let Some(window) = window else {
                return;
            };
            let rect = window.bounds_in_screen();
            let bounds = BrowserBounds {
                x: rect.x,
                y: rect.y,
                width: rect.width,
                height: rect.height,
            };
            let state = if window.is_maximized() == 1 {
                WindowState::Maximized
            } else if window.is_minimized() == 1 {
                WindowState::Minimized
            } else {
                WindowState::Normal
            };
            for delegate in &self.closing {
                delegate.on_window_closing(bounds, state);
            }
        }

        fn on_window_destroyed(&self, _window: Option<&mut Window>) {
            debug!("Window destroyed");
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
                return host.try_close_browser();
            }
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
                    Placement::Popup(requested),
                    Vec::new(),
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

/// Opens `url` in a new browser, in a new top-level window at `placement`,
/// whose closing is told to `closing`.
/// UI thread, where CEF creates browsers and windows.
///
/// The browser is created when the window adds its view
/// (cef_browser_view.h:53-54), and is registered and linked to the window
/// then.
pub(crate) fn open_browser_window(
    app: &AppHandle,
    url: &str,
    placement: Placement,
    closing: Vec<Arc<dyn ClientAppBrowserDelegate>>,
) -> Result<WindowId, RuntimeError> {
    let mut client = KuroganeClient::new(app.clone(), BrowserType::Main);
    // The guard ends with the statement, before any CEF call
    let window_id = app.registry().windows.allocate_id();
    let mut view_delegate = KuroganeBrowserViewDelegate::new(app.clone(), Some(window_id));

    debug!(
        "Creating a browser for {url} in window {}",
        window_id.as_u32()
    );
    let browser_view = browser_view_create(
        Some(&mut client),
        Some(&CefString::from(url)),
        Some(&Default::default()),
        None,
        None,
        Some(&mut view_delegate),
    )
    .ok_or(RuntimeError::BrowserCreationFailed)?;

    let mut delegate = KuroganeWindowDelegate::new(
        window_id,
        browser_view,
        app.clone(),
        None,
        placement,
        closing,
    );
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

    #[test]
    fn an_application_window_opens_at_its_bounds_in_its_state() {
        let placement = Placement::Main {
            bounds: Rect {
                x: 10,
                y: 20,
                width: 800,
                height: 600,
            },
            show_state: ShowState::HIDDEN,
        };
        let Rect {
            x,
            y,
            width,
            height,
        } = placement.initial_bounds();
        assert_eq!((x, y, width, height), (10, 20, 800, 600));
        // Sized by its bounds, or by CEF when they are empty
        let Size { width, height } = placement.preferred_size();
        assert_eq!((width, height), (0, 0));
        assert_eq!(placement.show_state(), ShowState::HIDDEN);
    }

    #[test]
    fn a_popup_opens_at_what_its_page_asked_for() {
        // A size alone: CEF places the window, at that size
        let sized = Placement::Popup(Some(size_only(320, 200)));
        let Rect { width, height, .. } = sized.initial_bounds();
        assert_eq!((width, height), (0, 0));
        let Size { width, height } = sized.preferred_size();
        assert_eq!((width, height), (320, 200));
        assert_eq!(sized.show_state(), ShowState::NORMAL);

        let placed = Placement::Popup(Some(PopupGeometry {
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
        let default = Placement::Popup(None);
        let Rect { width, height, .. } = default.initial_bounds();
        assert_eq!((width, height), (0, 0));
        let Size { width, height } = default.preferred_size();
        assert_eq!((width, height), (0, 0));
        assert_eq!(default.show_state(), ShowState::NORMAL);
    }
}

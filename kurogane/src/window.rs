//! Native window delegate.
//!
//! Controls how the native window behaves and embeds the
//! browser view into the platform window.

use cef::*;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::debug;
use crate::browser_registry::{BrowserId, BrowserRegistry, BrowserType};
use crate::window_registry::WindowRegistry;
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

wrap_window_delegate! {
    pub struct KuroganeWindowDelegate {
        window_id: WindowId,
        browser_view: BrowserView,
        registry: Arc<Mutex<WindowRegistry>>,
        initial_bounds: Rect,
        show_state: ShowState,
        is_closing: Arc<AtomicBool>,
    }

    impl ViewDelegate {
        fn on_child_view_changed(
            &self,
            _view: Option<&mut View>,
            _added: ::std::os::raw::c_int,
            _child: Option<&mut View>,
        ) {
            // Intentionally unused
        }
    }

    impl PanelDelegate {}

    impl WindowDelegate {
        fn initial_bounds(&self, _window: Option<&mut Window>) -> Rect {
            self.initial_bounds.clone()
        }

        fn initial_show_state(&self, _window: Option<&mut Window>) -> ShowState {
            self.show_state
        }

        fn on_window_created(&self, window: Option<&mut Window>) {
            if let Some(window) = window {
                // Registered before the BrowserView is added, which creates
                // its browser; on_browser_created links that browser here
                let mut reg = self.registry.lock().unwrap();
                reg.insert(
                    self.window_id,
                    window.clone(),
                    None,
                );
                drop(reg);

                let view = self.browser_view.clone();
                window.add_child_view(Some(&mut (&view).into()));
                if self.show_state != ShowState::HIDDEN {
                    window.show();
                }
                debug!("Window shown");
            }
        }

        fn on_window_destroyed(&self, _window: Option<&mut Window>) {
            debug!("Window destroyed");

            let mut reg = self.registry.lock().unwrap();
            reg.unregister(self.window_id);
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
            if self.is_closing.load(Ordering::Acquire) {
                return 1;
            }
            if let Some(browser) = self.browser_view.browser() && let Some(host) = browser.host() {
                return host.try_close_browser();
            }
            1
        }
    }
}

wrap_browser_view_delegate! {
    pub struct KuroganeBrowserViewDelegate {
        registry: Arc<Mutex<BrowserRegistry>>,
        window_registry: Arc<Mutex<WindowRegistry>>,
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
                .registry
                .lock()
                .unwrap()
                .ensure_registered(browser, BrowserType::Main, None);

            if self.window_registry.lock().unwrap().link(window_id, browser_id) {
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
            Some(KuroganeBrowserViewDelegate::new(
                self.registry.clone(),
                self.window_registry.clone(),
                None,
            ))
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
                        let reg = self.registry.lock().unwrap();
                        reg.find_id_by_browser(&b)
                    });

                // Classified here, where its kind and opener are known,
                // whether or not on_after_created registered it first
                let browser_type = if is_devtools != 0 { BrowserType::DevTools } else { BrowserType::Popup };
                let browser_id = pbv.browser().map(|browser| {
                    let mut reg = self.registry.lock().unwrap();
                    let id = reg.ensure_registered(&browser, browser_type, parent_id);
                    reg.classify(id, browser_type, parent_id);
                    debug!("[BrowserViewDelegate] registered popup browser");
                    id
                });

                // What the page asked of the window, kept by the opener since
                // on_before_popup. DevTools popups come from
                // on_before_dev_tools_popup and are not in its list
                let requested = match parent_id {
                    Some(opener) if is_devtools == 0 => self
                        .registry
                        .lock()
                        .unwrap()
                        .get_mut(opener)
                        .and_then(|state| state.pending_popups.take()),
                    _ => None,
                };
                debug!("[BrowserViewDelegate] popup window requested {:?}", requested);

                // Create the popup window with a delegate that tracks the window
                let bv_clone = pbv.clone();
                let window_id = {
                    let mut reg = self.window_registry.lock().unwrap();
                    reg.allocate_id()
                };

                let is_closing = Arc::new(AtomicBool::new(false));
                let mut delegate = KuroganePopupDelegate::new(
                    window_id,
                    bv_clone,
                    self.window_registry.clone(),
                    browser_id,
                    requested,
                    ShowState::NORMAL,
                    is_closing,
                );
                if let Some(window) = window_create_top_level(Some(&mut delegate)) {
                    window.show();
                    debug!("[BrowserViewDelegate] popup window created and shown");
                    return 1;
                }
            }

            0
        }
    }
}

wrap_window_delegate! {
    pub struct KuroganePopupDelegate {
        window_id: WindowId,
        browser_view: BrowserView,
        registry: Arc<Mutex<WindowRegistry>>,
        browser_id: Option<BrowserId>,
        // The window the page asked for; without it CEF gives the popup its
        // default 800x600 window
        requested: Option<PopupGeometry>,
        show_state: ShowState,
        is_closing: Arc<AtomicBool>,
    }

    impl ViewDelegate {
        fn preferred_size(&self, _view: Option<&mut View>) -> Size {
            self.requested.map(PopupGeometry::size).unwrap_or_default()
        }
    }

    impl PanelDelegate {}

    impl WindowDelegate {
        // Empty unless the page placed the window; CEF then places it and
        // takes its size from preferred_size
        fn initial_bounds(&self, _window: Option<&mut Window>) -> Rect {
            self.requested.and_then(PopupGeometry::bounds).unwrap_or_default()
        }

        fn initial_show_state(&self, _window: Option<&mut Window>) -> ShowState {
            self.show_state
        }

        fn on_window_created(&self, window: Option<&mut Window>) {
            if let Some(window) = window {
                let view = self.browser_view.clone();
                window.add_child_view(Some(&mut (&view).into()));
                if self.show_state != ShowState::HIDDEN {
                    window.show();
                }
                debug!("Popup window shown at {:?}", window.bounds());

                // Register popup window in registry, associated with its browser
                let mut reg = self.registry.lock().unwrap();
                reg.insert(
                    self.window_id,
                    window.clone(),
                    self.browser_id,
                );
            }
        }

        fn on_window_destroyed(&self, _window: Option<&mut Window>) {
            debug!("Popup window destroyed");

            let mut reg = self.registry.lock().unwrap();
            reg.unregister(self.window_id);
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
            if self.is_closing.load(Ordering::Acquire) {
                return 1;
            }
            if let Some(browser) = self.browser_view.browser() && let Some(host) = browser.host() {
                return host.try_close_browser();
            }
            1
        }
    }
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
}

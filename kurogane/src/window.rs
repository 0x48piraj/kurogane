//! Native window delegate.
//!
//! Controls how the native window behaves and embeds the
//! browser view into the platform window.

use cef::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::debug;
use crate::browser_registry::{BrowserId, BrowserRegistry, BrowserType};
use crate::window_registry::WindowRegistry;
use crate::window_registry::WindowId;

/// A string that survives cef-rs writing an out-parameter struct back to CEF:
/// that conversion drops any `CefString` it did not borrow from CEF, so the
/// buffer is allocated through CEF itself (destructor attached) and freed by CEF.
/// What the window manager sees of a window: its class and its title.
#[derive(Clone, Debug, Default)]
pub(crate) struct WindowIdentity {
    /// WM_CLASS under X11, app_id under Wayland. Linux only; other platforms ignore it.
    pub class: Option<String>,
    /// The native title. None leaves the window untitled.
    pub title: Option<String>,
    /// The native icon, an encoded PNG. None leaves the platform's default.
    pub icon: Option<Vec<u8>>,
}

/// A CEF image decoded from a PNG, or None when CEF could not decode it.
fn cef_image(png: &[u8]) -> Option<Image> {
    let image = image_create()?;
    (image.add_png(1.0, Some(png)) == 1).then_some(image)
}

fn cef_owned_string(value: &str) -> CefString {
    let utf16: Vec<u16> = value.encode_utf16().collect();
    let mut raw: sys::_cef_string_utf16_t = unsafe { std::mem::zeroed() };
    // SAFETY: `utf16` outlives the call, and copy = 1 makes CEF allocate its own buffer.
    unsafe { sys::cef_string_utf16_set(utf16.as_ptr(), utf16.len(), &mut raw, 1) };
    CefString::from(raw)
}

wrap_window_delegate! {
    pub struct KuroganeWindowDelegate {
        window_id: WindowId,
        browser_view: BrowserView,
        registry: Arc<Mutex<WindowRegistry>>,
        initial_bounds: Rect,
        show_state: ShowState,
        is_closing: Arc<AtomicBool>,
        identity: WindowIdentity,
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
        fn linux_window_properties(
            &self,
            _window: Option<&mut Window>,
            properties: Option<&mut LinuxWindowProperties>,
        ) -> ::std::os::raw::c_int {
            let (Some(class), Some(properties)) = (&self.identity.class, properties) else {
                return 0;
            };
            properties.wayland_app_id = cef_owned_string(class);
            properties.wm_class_class = cef_owned_string(class);
            properties.wm_class_name = cef_owned_string(class);
            1
        }

        fn initial_bounds(&self, _window: Option<&mut Window>) -> Rect {
            self.initial_bounds.clone()
        }

        fn initial_show_state(&self, _window: Option<&mut Window>) -> ShowState {
            self.show_state
        }

        fn on_window_created(&self, window: Option<&mut Window>) {
            if let Some(window) = window {
                // Register window first so on_after_created can find and link it
                let mut reg = self.registry.lock().unwrap();
                reg.insert(
                    self.window_id,
                    window.clone(),
                    None,
                );
                drop(reg);

                let view = self.browser_view.clone();
                window.add_child_view(Some(&mut (&view).into()));
                if let Some(title) = &self.identity.title {
                    window.set_title(Some(&CefString::from(title.as_str())));
                }
                // One image for both: the app icon is what the taskbar and
                // the switcher draw, the window icon what the title bar does,
                // and CEF scales each from it.
                if let Some(mut image) = self.identity.icon.as_deref().and_then(cef_image) {
                    window.set_window_icon(Some(&mut image));
                    window.set_window_app_icon(Some(&mut image));
                }
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
    }

    impl ViewDelegate {}

    impl BrowserViewDelegate {
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

                // Register the popup browser before it hits on_after_created
                let browser_type = if is_devtools != 0 { BrowserType::DevTools } else { BrowserType::Popup };
                let browser_id = if let Some(browser) = pbv.browser() {
                    let mut reg = self.registry.lock().unwrap();
                    let id = reg.register(browser.clone(), browser_type, parent_id);
                    if let Some(pid) = parent_id {
                        reg.set_opener(id, Some(pid));
                    }
                    debug!("[BrowserViewDelegate] registered popup browser");
                    Some(id)
                } else {
                    None
                };

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
        show_state: ShowState,
        is_closing: Arc<AtomicBool>,
    }

    impl ViewDelegate {}

    impl PanelDelegate {}

    impl WindowDelegate {
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
                debug!("Popup window shown");

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

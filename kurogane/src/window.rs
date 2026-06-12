//! Native window delegate.
//!
//! Controls how the native window behaves and embeds the
//! browser view into the platform window.

use cef::*;
use std::sync::{Arc, Mutex};

use crate::debug;
use crate::browser_registry::{BrowserId, BrowserRegistry, BrowserType};
use crate::window_registry::WindowRegistry;
use crate::window_registry::WindowId;

wrap_window_delegate! {
    pub struct KuroganeWindowDelegate {
        window_id: WindowId,
        browser_view: BrowserView,
        registry: Arc<Mutex<WindowRegistry>>,
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
                window.show();
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

                let mut delegate = KuroganePopupDelegate::new(
                    window_id,
                    bv_clone,
                    self.window_registry.clone(),
                    browser_id,
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
    }

    impl ViewDelegate {}

    impl PanelDelegate {}

    impl WindowDelegate {
        fn on_window_created(&self, window: Option<&mut Window>) {
            if let Some(window) = window {
                let view = self.browser_view.clone();
                window.add_child_view(Some(&mut (&view).into()));
                window.show();
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
            1
        }
    }
}

//! Native window delegate.
//!
//! Controls how the native window behaves and embeds the
//! browser view into the platform window.

use cef::*;
use std::sync::{Arc, Mutex};

wrap_window_delegate! {
    pub struct DemoWindowDelegate {
        browser_view: BrowserView,
        window_ref: Arc<Mutex<Option<Window>>>,
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
                let view = self.browser_view.clone();
                window.add_child_view(Some(&mut (&view).into()));
                window.show();

                // store live window reference so the browser process keeps
                // an owning handle and we can clear it on destroy
                *self.window_ref.lock().unwrap() = Some(window.clone());
            }
        }

        fn on_window_destroyed(&self, _window: Option<&mut Window>) {
            // clear stored window reference to avoid use-after-destroy
            *self.window_ref.lock().unwrap() = None;
            quit_message_loop();
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

//! The window CEF makes for a browser embedded with
//! [`AppInstance::create_child_browser`](crate::AppInstance::create_child_browser).
//!
//! CEF makes that window a child of the host's window. Once such a browser is
//! ready to close, CEF's default asks the host's top-level window to close
//! (`WM_CLOSE` on Windows, `performClose:` on macOS) and finishes only when a
//! window is destroyed. Kurogane destroys the browser's own window instead,
//! which completes the close (`CefLifeSpanHandler::DoClose`). Linux needs
//! neither: CEF closes the browser's own X window there.

use cef::sys::cef_window_handle_t;
use cef::*;

use crate::debug;

/// Destroys `browser`'s own window once the running CEF callback returns.
///
/// Destroying it re-enters CEF's close, so it cannot happen inside
/// `DoClose`. Returns false when the task could not be posted.
pub(crate) fn destroy_child_window_later(browser: &Browser) -> bool {
    let mut task = DestroyChildWindow::new(browser.clone());
    post_task(ThreadId::UI, Some(&mut task)) != 0
}

wrap_task! {
    struct DestroyChildWindow {
        browser: Browser,
    }

    impl Task {
        fn execute(&self) {
            // Closed already if the host destroyed its own window first; the
            // handle would then name a window that is gone
            if self.browser.is_valid() == 0 {
                return;
            }
            if let Some(host) = self.browser.host() {
                debug!("destroying the window of embedded browser cef_id={}", self.browser.identifier());
                destroy(host.window_handle());
            }
        }
    }
}

/// Destroys CEF's `CefBrowserWindow`; its `WM_NCDESTROY` finishes the close.
#[cfg(target_os = "windows")]
fn destroy(window: cef_window_handle_t) {
    use windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow;

    if window.0.is_null() {
        return;
    }
    // SAFETY: this runs after DoClose, past the page's unload handlers, so a
    // destruction of this window since then (its WM_NCDESTROY) would already
    // have finished the close and made the browser invalid, which the caller
    // checked on this thread. The handle is therefore still the browser's own
    // window, which CEF created on this thread, CEF's UI thread; DestroyWindow
    // must run on the thread that made the window.
    unsafe {
        DestroyWindow(window.0.cast());
    }
}

/// Takes CEF's `CefBrowserHostView` out of the host's view, which holds its
/// only reference; the view's `dealloc` finishes the close.
#[cfg(target_os = "macos")]
fn destroy(window: cef_window_handle_t) {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;

    let view = window.cast::<AnyObject>();
    if view.is_null() {
        return;
    }
    // SAFETY: this runs after DoClose, past the page's unload handlers, so if
    // the view had been deallocated since then (its dealloc reports the
    // window destroyed) the close would already have finished and made the
    // browser invalid, which the caller checked on this thread. The view is
    // therefore alive. This is the main thread, which is CEF's UI thread on
    // macOS and where AppKit views live.
    unsafe {
        let _: () = msg_send![view, removeFromSuperview];
    }
}

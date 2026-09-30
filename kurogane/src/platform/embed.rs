//! The window CEF makes for a browser embedded with
//! [`AppInstance::create_child_browser`](crate::AppInstance::create_child_browser):
//! a native child window of the application's own window.
//!
//! Closing. Once such a browser is ready to close, CEF's default asks the
//! host's top-level window to close (`WM_CLOSE` on Windows, `performClose:` on
//! macOS) and finishes only when a window is destroyed. Kurogane destroys the
//! browser's own window instead, which completes the close
//! (`CefLifeSpanHandler::DoClose`). Linux needs neither: CEF closes the
//! browser's own X window there.
//!
//! Placing. CEF places the window once, at its creation bounds, and has no
//! call that moves it: `CefBrowserHost::WasResized` is only for windowless
//! browsers (cef_browser.h). [`BrowserHandle::set_bounds`](crate::BrowserHandle::set_bounds)
//! moves it the way CEF's sample client does on Windows and X11
//! (`BrowserWindowStd*::SetBounds`). On macOS CEF makes the view stretch with
//! its parent; setting its frame places it anywhere.
//!
//! Everything here runs on CEF's UI thread.

use cef::sys::cef_window_handle_t;

use crate::browser_registry::BrowserId;
use crate::chromium_flags::ChromiumFlags;
use crate::runtime::BrowserBounds;
use crate::spec::RuntimeMode;

#[cfg(any(target_os = "windows", target_os = "macos"))]
pub(crate) use close::destroy_child_window_later;

#[cfg(target_os = "macos")]
pub(crate) use imp::{forget_view, remember_view};

/// Runs Chromium on X11 in an embedded application on Linux.
///
/// CEF parents an embedded browser to an X11 window there, and Chromium
/// draws into it only on its X11 backend. In a Wayland session Chromium picks
/// Wayland by itself: the page goes to a surface of its own beside the host's
/// window, and the browser's close never completes.
/// `App::chromium_flag_with_value("ozone-platform", ..)` still overrides this;
/// as with every switch the runtime sets, the process's own command line does
/// not.
pub(crate) fn apply_embedding_flags(flags: &mut ChromiumFlags, mode: RuntimeMode) {
    if cfg!(target_os = "linux") && mode == RuntimeMode::Embedded {
        flags.set_with_value("ozone-platform", "x11");
    }
}

/// Moves and resizes browser `id`'s window, `handle`, inside its parent, in
/// the parent's coordinates, as CEF placed it at creation.
pub(crate) fn set_child_window_bounds(
    id: BrowserId,
    handle: cef_window_handle_t,
    bounds: BrowserBounds,
) {
    // X11 has no empty windows, and a negative size means nothing anywhere
    let bounds = BrowserBounds {
        width: bounds.width.max(1),
        height: bounds.height.max(1),
        ..bounds
    };
    imp::set_child_window_bounds(id, handle, bounds);
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
mod close {
    use cef::*;

    use super::cef_window_handle_t;
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
                // Closed already if the host destroyed its own window first;
                // the handle would then name a window that is gone
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
        // SAFETY: this runs after DoClose, past the page's unload handlers, so
        // a destruction of this window since then (its WM_NCDESTROY) would
        // already have finished the close and made the browser invalid, which
        // the caller checked on this thread. The handle is therefore still the
        // browser's own window, which CEF created on this thread, CEF's UI
        // thread; DestroyWindow must run on the thread that made the window.
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
        // SAFETY: this runs after DoClose, past the page's unload handlers, so
        // if the view had been deallocated since then (its dealloc reports the
        // window destroyed) the close would already have finished and made the
        // browser invalid, which the caller checked on this thread. The view is
        // therefore alive. This is the main thread, which is CEF's UI thread on
        // macOS and where AppKit views live.
        unsafe {
            let _: () = msg_send![view, removeFromSuperview];
        }
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPos};

    use super::{BrowserBounds, BrowserId, cef_window_handle_t};

    /// cefclient's `BrowserWindowStdWin::SetBounds`.
    pub(super) fn set_child_window_bounds(
        _id: BrowserId,
        handle: cef_window_handle_t,
        bounds: BrowserBounds,
    ) {
        if handle.0.is_null() {
            return;
        }
        // SAFETY: SetWindowPos reads no memory through the handle: user32
        // checks it, so a handle whose window is already gone only fails the
        // call. Until the browser closes it is the browser's own window, made
        // on this thread, CEF's UI thread. SWP_NOZORDER leaves the null
        // insert-after handle unused.
        unsafe {
            SetWindowPos(
                handle.0.cast(),
                std::ptr::null_mut(),
                bounds.x,
                bounds.y,
                bounds.width,
                bounds.height,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use x11_dl::xlib::{Display, Xlib};

    use super::{BrowserBounds, BrowserId, cef_window_handle_t};

    /// cefclient's `BrowserWindowStdGtk::SetBounds`, on CEF's own X connection.
    pub(super) fn set_child_window_bounds(
        _id: BrowserId,
        handle: cef_window_handle_t,
        bounds: BrowserBounds,
    ) {
        // Null off CEF's UI thread
        let display = cef::get_xdisplay().cast::<Display>();
        if handle == 0 || display.is_null() {
            return;
        }
        // libX11, which Chromium has loaded already; x11-dl opens it once
        let Ok(xlib) = Xlib::open() else {
            return;
        };
        // SAFETY: `display` is CEF's live Xlib connection, used on CEF's UI
        // thread as cef_get_xdisplay requires. `handle` is the browser's X
        // window id, which the server checks; an error goes to the handler
        // Chromium installs on this libX11, which logs it. The size is at
        // least one, so the casts keep it.
        unsafe {
            (xlib.XMoveResizeWindow)(
                display,
                handle,
                bounds.x,
                bounds.y,
                bounds.width as u32,
                bounds.height as u32,
            );
            // Xlib buffers requests until a flush
            (xlib.XFlush)(display);
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use objc2::rc::{Retained, Weak};
    use objc2_app_kit::NSView;

    use super::{BrowserBounds, BrowserId, cef_window_handle_t};

    thread_local! {
        /// The view of each browser made by create_child_browser, held
        /// weakly. Only its parent keeps CEF's view alive, and it can be
        /// freed before the browser's OnBeforeClose, so the browser's handle
        /// alone could name a freed object.
        static VIEWS: RefCell<HashMap<BrowserId, Weak<NSView>>> = RefCell::new(HashMap::new());
    }

    /// Records browser `id`'s view, `handle`, right after CEF made it.
    pub(crate) fn remember_view(id: BrowserId, handle: cef_window_handle_t) {
        // SAFETY: CEF has just made this view for the browser and added it to
        // its parent, which keeps it alive; this is the main thread, where
        // AppKit views live. A null handle gives None.
        let Some(view) = (unsafe { Retained::retain(handle.cast::<NSView>()) }) else {
            return;
        };
        VIEWS.with(|views| {
            views.borrow_mut().insert(id, Weak::from_retained(&view));
        });
    }

    /// Forgets a closed browser's view.
    pub(crate) fn forget_view(id: BrowserId) {
        VIEWS.with(|views| {
            views.borrow_mut().remove(&id);
        });
    }

    /// Sets the view's frame in the parent's points. CEF used the creation
    /// bounds as this frame the same way, unconverted.
    pub(super) fn set_child_window_bounds(
        id: BrowserId,
        _handle: cef_window_handle_t,
        bounds: BrowserBounds,
    ) {
        let Some(view) = VIEWS.with(|views| views.borrow().get(&id).and_then(Weak::load)) else {
            return;
        };
        let mut frame = view.frame();
        frame.origin.x = bounds.x.into();
        frame.origin.y = bounds.y.into();
        frame.size.width = bounds.width.into();
        frame.size.height = bounds.height.into();
        view.setFrame(frame);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chromium_flags::ChromiumFlag;

    #[test]
    fn only_an_embedded_application_on_linux_asks_for_x11() {
        let mut views = ChromiumFlags::default();
        apply_embedding_flags(&mut views, RuntimeMode::Views);
        assert_eq!(views.to_string(), "");

        let mut embedded = ChromiumFlags::default();
        apply_embedding_flags(&mut embedded, RuntimeMode::Embedded);
        let expected = if cfg!(target_os = "linux") {
            "--ozone-platform=x11\n"
        } else {
            ""
        };
        assert_eq!(embedded.to_string(), expected);
    }

    #[test]
    fn a_user_ozone_platform_overrides_it() {
        let mut flags = ChromiumFlags::default();
        apply_embedding_flags(&mut flags, RuntimeMode::Embedded);
        flags.extend_user_flags(&[ChromiumFlag::WithValue(
            "--ozone-platform".into(),
            "wayland".into(),
        )]);
        assert_eq!(flags.to_string(), "--ozone-platform=wayland\n");
    }
}

//! Xlib host, a window from `XCreateSimpleWindow` and a loop over its connection.
//!
//! The loop polls the X connection and a socket. The scheduler writes a byte to
//! the socket from any thread to wake the loop. Chromium parents its browser to
//! the window through its own X connection, under XWayland in a Wayland
//! session.

use std::ffi::{CStr, c_int, c_uint};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::ptr::null;

use kurogane::raw_window_handle::{
    HandleError, HasWindowHandle, RawWindowHandle, WindowHandle, XlibWindowHandle,
};
use kurogane_suite::embedding;
use x11_dl::xlib;

/// What the loop takes from the window's events.
enum WindowEvent {
    /// The window manager asks to close the window.
    CloseRequested,
    /// The window's size or place, new or not.
    Configured { width: i32, height: i32 },
    /// Any other event.
    Other,
}

/// Host window on an X connection of its own, used on the thread that opened
/// it.
struct HostWindow {
    x: xlib::Xlib,
    display: *mut xlib::Display,
    id: xlib::Window,
    wm_protocols: xlib::Atom,
    wm_delete_window: xlib::Atom,
}

impl HostWindow {
    /// Opens a window of `width` by `height` titled `title` and maps it.
    fn open(title: &CStr, width: c_uint, height: c_uint) -> Self {
        let x = xlib::Xlib::open().expect("libX11 loads");
        // SAFETY: a display opened here and used on this thread only; the
        // strings outlive the calls that take them
        unsafe {
            let display = (x.XOpenDisplay)(null());
            assert!(!display.is_null(), "no X display; set DISPLAY");
            let root = (x.XDefaultRootWindow)(display);
            let id = (x.XCreateSimpleWindow)(display, root, 0, 0, width, height, 0, 0, 0);
            (x.XSelectInput)(display, id, xlib::StructureNotifyMask);
            (x.XStoreName)(display, id, title.as_ptr());

            // The window manager asks to close the window instead of
            // destroying it
            let wm_protocols = (x.XInternAtom)(display, c"WM_PROTOCOLS".as_ptr(), xlib::False);
            let mut wm_delete_window =
                (x.XInternAtom)(display, c"WM_DELETE_WINDOW".as_ptr(), xlib::False);
            (x.XSetWMProtocols)(display, id, &mut wm_delete_window, 1);

            (x.XMapWindow)(display, id);
            // The window exists on the server before Chromium parents to it
            (x.XSync)(display, xlib::False);
            Self {
                x,
                display,
                id,
                wm_protocols,
                wm_delete_window,
            }
        }
    }

    /// Returns the X connection's descriptor, readable when events arrive.
    fn connection(&self) -> RawFd {
        // SAFETY: the display is open and this thread's
        unsafe { (self.x.XConnectionNumber)(self.display) }
    }

    /// Returns the next event already received, without waiting.
    fn next_event(&self) -> Option<WindowEvent> {
        // SAFETY: the display is open and this thread's; each union field is
        // read only as the type the event's type names
        unsafe {
            if (self.x.XPending)(self.display) == 0 {
                return None;
            }
            let mut event: xlib::XEvent = std::mem::zeroed();
            (self.x.XNextEvent)(self.display, &mut event);
            Some(match event.get_type() {
                xlib::ClientMessage if self.asks_to_close(&event.client_message) => {
                    WindowEvent::CloseRequested
                }
                xlib::ConfigureNotify => WindowEvent::Configured {
                    width: event.configure.width,
                    height: event.configure.height,
                },
                _ => WindowEvent::Other,
            })
        }
    }

    /// Returns whether `message` is the window manager's request to close the
    /// window.
    fn asks_to_close(&self, message: &xlib::XClientMessageEvent) -> bool {
        // An atom travels in a long
        message.message_type == self.wm_protocols
            && message.data.get_long(0) as xlib::Atom == self.wm_delete_window
    }
}

impl HasWindowHandle for HostWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let raw = RawWindowHandle::Xlib(XlibWindowHandle::new(self.id));
        // SAFETY: the window lives as long as self, which the host drops only
        // after its browser has closed
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}

impl Drop for HostWindow {
    fn drop(&mut self) {
        // SAFETY: the window and display are open and this thread's
        unsafe {
            (self.x.XDestroyWindow)(self.display, self.id);
            (self.x.XCloseDisplay)(self.display);
        }
    }
}

/// Runs the Xlib host until its window has closed.
pub fn run() {
    let (waiting, waker) = UnixStream::pair().expect("a socket pair wakes the loop");
    // Neither end ever blocks a thread of CEF's or the loop
    waiting
        .set_nonblocking(true)
        .expect("the socket turns non-blocking");
    waker
        .set_nonblocking(true)
        .expect("the socket turns non-blocking");
    let (runtime, schedule) = embedding::start(move || {
        // A full socket already wakes the loop
        let _ = (&waker).write(&[1]);
    })
    .expect("Kurogane failed to start");

    let window = HostWindow::open(c"Kurogane in Xlib", 1000, 700);
    let browser = runtime
        .create_child_browser(
            &window,
            embedding::filling(1000, 700),
            &embedding::page("Xlib"),
        )
        .inspect_err(|e| eprintln!("the browser could not be created:\n{e}"))
        .ok();

    while !runtime.should_shutdown() {
        wait_readable(
            [window.connection(), waiting.as_raw_fd()],
            schedule.wait_ms(),
        );
        // The bytes only woke the loop
        while (&waiting).read(&mut [0; 64]).is_ok_and(|read| read > 0) {}

        while let Some(event) = window.next_event() {
            match event {
                WindowEvent::CloseRequested => {
                    // Close the browser first; the window stays until it has
                    runtime.handle().close_all_browsers(true);
                }
                WindowEvent::Configured { width, height } => {
                    // Chromium placed the browser once; keep it filling the
                    // window. The browser learns of the window's place only
                    // from the notice; its popups and the page's screen
                    // position follow it
                    if let Some(browser) = &browser {
                        browser.set_bounds(embedding::filling(width, height));
                        browser.notify_move_or_resize_started();
                    }
                }
                WindowEvent::Other => {}
            }
        }

        schedule.pump_if_due(&runtime);
    }

    // The browser is gone, whatever closed it; the window goes first
    drop(window);
    runtime.shutdown();
}

/// Waits until one of `fds` is readable or `timeout_ms` has passed.
fn wait_readable(fds: [RawFd; 2], timeout_ms: u32) {
    let mut polled = fds.map(|fd| libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    });
    let timeout = c_int::try_from(timeout_ms).unwrap_or(c_int::MAX);
    // SAFETY: polled holds a valid pollfd for each of its entries
    unsafe { libc::poll(polled.as_mut_ptr(), polled.len() as libc::nfds_t, timeout) };
}

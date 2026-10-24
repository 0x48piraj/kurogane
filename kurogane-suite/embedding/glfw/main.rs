//! Chromium as a child browser in a window GLFW makes and an event loop GLFW runs.
//!
//! GLFW's window hands its native handle to Kurogane through
//! raw-window-handle. It makes no GL context; Chromium draws the window's
//! content. On Linux GLFW runs on X11, where Chromium needs its parent. On
//! macOS it keeps Kurogane's App, Edit and Window menus.
//!
//! Chromium is pumped at the earliest deadline it asked for and at least every
//! 33 ms. The scheduler posts GLFW's empty event from any thread to wake the
//! loop.
//!
//! Windows runs a loop of its own while the user moves or resizes the window
//! or opens its menu. GLFW's loop waits until it ends. GLFW reports none of
//! those loops; the host wraps GLFW's window procedure. A timer pumps Chromium
//! from the start of each loop to its end and each `WM_SIZE` places the
//! browser. On macOS AppKit runs one while the user resizes the window or
//! tracks a menu; a timer in the run loop's common modes pumps Chromium inside
//! it.
//!
//! Closing the window closes the browser first. The loop keeps pumping until
//! should_shutdown() turns true, then drops the window and shuts Kurogane
//! down.

use std::cell::OnceCell;
use std::sync::{Arc, Mutex, OnceLock};

use glfw::{
    ClientApiHint, Glfw, GlfwReceiver, PWindow, ThreadSafeGlfw, WindowEvent, WindowHint, WindowMode,
};
use kurogane::{AppInstance, BrowserBounds, BrowserHandle};
#[cfg(target_os = "macos")]
use kurogane_suite::embedding::PumpTimer;
use kurogane_suite::embedding::{self, PumpSchedule};

/// What GLFW's loop, the wrapped window procedure and the pump timer share on
/// the main thread.
struct Host {
    runtime: AppInstance,
    schedule: PumpSchedule,
    browser: Option<BrowserHandle>,
    /// macOS timer that pumps inside AppKit's own loops.
    #[cfg(target_os = "macos")]
    timer: PumpTimer,
}

thread_local! {
    /// The host while GLFW's loop runs, which the wrapped window procedure on
    /// Windows and the pump timer on macOS reach.
    static HOST: OnceCell<Host> = const { OnceCell::new() };
}

/// Runs `f` with the host once GLFW's loop runs.
#[cfg(any(windows, target_os = "macos"))]
fn with_host(f: impl FnOnce(&Host)) {
    HOST.with(|host| {
        if let Some(host) = host.get() {
            f(host);
        }
    });
}

impl Host {
    /// Runs GLFW's loop until the browser has closed, then drops the window and
    /// shuts Kurogane down.
    fn run(&self, glfw: &mut Glfw, mut window: PWindow, events: &GlfwReceiver<(f64, WindowEvent)>) {
        while !self.runtime.should_shutdown() {
            glfw.wait_events_timeout(self.schedule.wait().as_secs_f64());
            for (_, event) in glfw::flush_messages(events) {
                match event {
                    WindowEvent::Close => {
                        // GLFW only flags the close; the window stays until the
                        // browser has closed
                        window.set_should_close(false);
                        self.runtime.handle().close_all_browsers(true);
                    }
                    // Chromium placed the browser once; keep it filling the
                    // window and tell it of each move. The wrapped window
                    // procedure does both on Windows
                    #[cfg(not(windows))]
                    WindowEvent::FramebufferSize(..) => self.place(client_bounds(&window)),
                    #[cfg(not(windows))]
                    WindowEvent::Pos(..) => self.moved(),
                    _ => {}
                }
            }
            self.pump_if_due();
        }

        // The browser is gone, whatever closed it; the window goes first
        #[cfg(target_os = "macos")]
        self.timer.disarm();
        drop(window);
        self.runtime.shutdown();
    }

    /// Pumps Chromium once a pump is due. On macOS it then arms the timer for
    /// the next deadline.
    fn pump_if_due(&self) {
        #[cfg(target_os = "macos")]
        self.timer.pump_if_due(&self.schedule, &self.runtime);
        #[cfg(not(target_os = "macos"))]
        self.schedule.pump_if_due(&self.runtime);
    }

    /// Places the browser at `bounds` in the window.
    fn place(&self, bounds: BrowserBounds) {
        if let Some(browser) = &self.browser {
            browser.set_bounds(bounds);
        }
    }

    /// Tells the browser its window moved. The browser learns of the window's
    /// place only from this; its popups and the page's screen position follow
    /// it.
    fn moved(&self) {
        if let Some(browser) = &self.browser {
            browser.notify_move_or_resize_started();
        }
    }
}

/// GLFW's window procedure, wrapped for the loops Windows runs of its own.
#[cfg(windows)]
mod modal {
    use std::cell::Cell;

    use kurogane_suite::embedding;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CallWindowProcW, GWLP_WNDPROC, KillTimer, SetTimer, SetWindowLongPtrW, USER_TIMER_MINIMUM,
        WM_ENTERMENULOOP, WM_ENTERSIZEMOVE, WM_EXITMENULOOP, WM_EXITSIZEMOVE, WM_MOVE, WM_SIZE,
        WM_TIMER, WNDPROC,
    };

    use super::{Host, with_host};

    /// Timer that pumps while Windows moves or resizes GLFW's window or shows
    /// its menu.
    const MODAL_TIMER: usize = 0x4b47;

    thread_local! {
        /// GLFW's own window procedure, which every message reaches.
        static GLFW_PROCEDURE: Cell<WNDPROC> = const { Cell::new(None) };
    }

    /// Wraps the window procedure of GLFW's `window`. Main thread.
    pub fn wrap(window: HWND) {
        let wrapper: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT = procedure;
        // SAFETY: window is GLFW's live window on this thread; the procedure
        // set has the signature Windows calls
        let previous =
            unsafe { SetWindowLongPtrW(window, GWLP_WNDPROC, wrapper as usize as isize) };
        // SAFETY: GWLP_WNDPROC's value is the previous procedure, a WNDPROC
        GLFW_PROCEDURE.set(unsafe { std::mem::transmute::<isize, WNDPROC>(previous) });
    }

    unsafe extern "system" fn procedure(
        window: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match message {
            WM_ENTERSIZEMOVE | WM_ENTERMENULOOP => {
                // SAFETY: window is this thread's; the timer has no callback
                unsafe { SetTimer(window, MODAL_TIMER, USER_TIMER_MINIMUM, None) };
            }
            WM_TIMER if wparam == MODAL_TIMER => {
                with_host(Host::pump_if_due);
                return 0;
            }
            WM_EXITSIZEMOVE | WM_EXITMENULOOP => {
                // SAFETY: window is this thread's and owns the timer
                unsafe { KillTimer(window, MODAL_TIMER) };
            }
            WM_SIZE => {
                // Chromium placed the browser once; keep it filling the
                // window's client area, whose pixels the message carries
                let (width, height) = ((lparam & 0xffff) as i32, ((lparam >> 16) & 0xffff) as i32);
                with_host(|host| host.place(embedding::filling(width, height)));
            }
            WM_MOVE => with_host(Host::moved),
            _ => {}
        }
        // SAFETY: GLFW's procedure, called with the message Windows passed in
        unsafe { CallWindowProcW(GLFW_PROCEDURE.get(), window, message, wparam, lparam) }
    }
}

fn main() {
    kurogane_suite::logging();

    // Kurogane starts before GLFW. On macOS it installs the NSApplication
    // subclass CEF needs; the wake reaches GLFW's loop once GLFW runs. The
    // lock makes GLFW's thread-safe handle, which is only Send, shareable by
    // every thread the scheduler runs on
    let waker: Arc<OnceLock<Mutex<ThreadSafeGlfw>>> = Arc::default();
    let (runtime, schedule) = embedding::start({
        let waker = Arc::clone(&waker);
        move || {
            if let Some(glfw) = waker.get()
                && let Ok(glfw) = glfw.lock()
            {
                glfw.post_empty_event();
            }
        }
    })
    .expect("Kurogane failed to start");

    // CEF parents a child browser to an X11 window on Linux, under XWayland in
    // a Wayland session
    #[cfg(target_os = "linux")]
    glfw::init_hint(glfw::InitHint::Platform(glfw::Platform::X11));
    // GLFW's own menu bar has no Edit menu; Cmd+C, Cmd+V and Cmd+A would
    // reach no page through it
    #[cfg(target_os = "macos")]
    glfw::init_hint(glfw::InitHint::CocoaMenubar(false));

    let mut glfw = glfw::init(glfw::fail_on_errors).expect("GLFW failed to start");
    let _ = waker.set(Mutex::new(ThreadSafeGlfw::from(&mut glfw)));

    // Chromium draws the window's content
    glfw.window_hint(WindowHint::ClientApi(ClientApiHint::NoApi));
    let (mut window, events) = glfw
        .create_window(1000, 700, "Kurogane in GLFW", WindowMode::Windowed)
        .expect("GLFW made no window");
    window.set_close_polling(true);
    window.set_framebuffer_size_polling(true);
    #[cfg(not(windows))]
    window.set_pos_polling(true);

    let browser = runtime
        .create_child_browser(&window, client_bounds(&window), &embedding::page("GLFW"))
        .inspect_err(|e| eprintln!("the browser could not be created:\n{e}"))
        .ok();

    #[cfg(windows)]
    modal::wrap(window.get_win32_window());

    let host = Host {
        runtime,
        schedule,
        browser,
        #[cfg(target_os = "macos")]
        timer: PumpTimer::new(
            objc2::MainThreadMarker::new().expect("GLFW runs on the main thread"),
            || with_host(Host::pump_if_due),
        ),
    };
    HOST.with(|cell| cell.get_or_init(|| host).run(&mut glfw, window, &events));
}

/// Returns the window's whole client area in the units a child browser is
/// placed in (points on macOS, pixels on Windows and X11).
fn client_bounds(window: &PWindow) -> BrowserBounds {
    #[cfg(target_os = "macos")]
    let (width, height) = window.get_size();
    #[cfg(not(target_os = "macos"))]
    let (width, height) = window.get_framebuffer_size();
    embedding::filling(width, height)
}

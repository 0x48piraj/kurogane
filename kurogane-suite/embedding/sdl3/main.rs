//! Chromium as a child browser in a window SDL3 makes and an event loop SDL3 runs.
//!
//! SDL's window hands its native handle to Kurogane through raw-window-handle.
//! On Linux SDL runs on X11, where Chromium needs its parent.
//!
//! Chromium is pumped at the earliest deadline it asked for and at least every
//! 33 ms. The scheduler pushes an SDL user event from any thread to wake the
//! loop.
//!
//! Windows and macOS run a loop of their own while the user moves or resizes
//! the window. SDL's loop waits until it ends. SDL keeps sending its exposed
//! event from inside that loop. An event watch pumps Chromium there.
//!
//! Closing the window closes the browser first. The loop keeps pumping until
//! should_shutdown() turns true, then drops the window and shuts Kurogane
//! down.

use std::cell::OnceCell;
use std::sync::{Arc, OnceLock};

use kurogane::{AppInstance, BrowserBounds, BrowserHandle};
use kurogane_suite::embedding::{self, PumpSchedule};
use sdl3::Sdl;
use sdl3::event::{Event, EventSender, WindowEvent};
use sdl3::video::Window;

/// User event that wakes SDL's loop.
struct Wake;

/// What SDL's loop and the event watch share on the main thread.
struct Host {
    runtime: AppInstance,
    schedule: PumpSchedule,
    browser: Option<BrowserHandle>,
}

thread_local! {
    /// The host once SDL's loop runs. SDL also calls the event watch on each
    /// thread that pushes an event, where this stays empty.
    static HOST: OnceCell<Host> = const { OnceCell::new() };
}

/// Runs `f` with the host, on the main thread once SDL's loop runs.
fn with_host(f: impl FnOnce(&Host)) {
    HOST.with(|host| {
        if let Some(host) = host.get() {
            f(host);
        }
    });
}

impl Host {
    /// Runs SDL's loop until the browser has closed, then drops the window and
    /// shuts Kurogane down.
    fn run(&self, sdl: &Sdl, window: Window) {
        let mut events = sdl.event_pump().expect("SDL has an event pump");
        while !self.runtime.should_shutdown() {
            let first = events.wait_event_timeout_ms(self.schedule.wait_ms());
            for event in first.into_iter().chain(events.poll_iter()) {
                match event {
                    // SDL's quit follows the last window's close request
                    Event::Window {
                        win_event: WindowEvent::CloseRequested,
                        ..
                    }
                    | Event::Quit { .. } => {
                        // Close the browser first; the window stays until it has
                        self.runtime.handle().close_all_browsers(true);
                    }
                    #[cfg(target_os = "macos")]
                    Event::Window {
                        win_event: WindowEvent::PixelSizeChanged(..),
                        ..
                    } => self.place(client_bounds(&window)),
                    _ => {}
                }
            }
            self.schedule.pump_if_due(&self.runtime);
        }

        // The browser is gone, whatever closed it; the window goes first
        drop(window);
        self.runtime.shutdown();
    }

    /// Answers each event as SDL sends it, inside the system's move and resize
    /// loop too.
    fn watch(&self, event: Event) {
        match event {
            Event::Window {
                win_event: WindowEvent::Exposed,
                ..
            } => self.schedule.pump_if_due(&self.runtime),
            // Chromium placed the browser once; keep it filling the window, in
            // pixels (the loop places it in points on macOS)
            #[cfg(not(target_os = "macos"))]
            Event::Window {
                win_event: WindowEvent::PixelSizeChanged(width, height),
                ..
            } => self.place(embedding::filling(width, height)),
            Event::Window {
                win_event: WindowEvent::Moved(..),
                ..
            } => self.moved(),
            _ => {}
        }
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

fn main() {
    kurogane_suite::logging();

    // Kurogane starts before SDL. On macOS it installs the NSApplication
    // subclass CEF needs; the wake reaches SDL's loop once SDL runs
    let sender: Arc<OnceLock<EventSender>> = Arc::default();
    let (runtime, schedule) = embedding::start({
        let sender = Arc::clone(&sender);
        move || {
            if let Some(sender) = sender.get() {
                let _ = sender.push_custom_event(Wake);
            }
        }
    })
    .expect("Kurogane failed to start");

    // CEF parents a child browser to an X11 window on Linux, under XWayland in
    // a Wayland session
    #[cfg(target_os = "linux")]
    sdl3::hint::set("SDL_VIDEO_DRIVER", "x11");
    // Chromium's window takes the X screen's default visual and fails under a
    // parent of another one. SDL picks a GL visual of its own unless told
    #[cfg(target_os = "linux")]
    sdl3::hint::set("SDL_VIDEO_X11_VISUALID", &default_visual_id().to_string());

    let sdl = sdl3::init().expect("SDL failed to start");
    let video = sdl.video().expect("SDL has no video");
    let events = sdl.event().expect("SDL has no events");
    events
        .register_custom_event::<Wake>()
        .expect("SDL registers the wake event");
    let _ = sender.set(events.event_sender());

    let window = video
        .window("Kurogane in SDL3", 1000, 700)
        .resizable()
        .build()
        .expect("SDL made no window");
    let browser = runtime
        .create_child_browser(&window, client_bounds(&window), &embedding::page("SDL3"))
        .inspect_err(|e| eprintln!("the browser could not be created:\n{e}"))
        .ok();

    let _watch = events.add_event_watch(|event: Event| with_host(|host| host.watch(event)));
    let host = Host {
        runtime,
        schedule,
        browser,
    };
    HOST.with(|cell| cell.get_or_init(|| host).run(&sdl, window));
}

/// Returns the X screen's default visual id.
#[cfg(target_os = "linux")]
fn default_visual_id() -> std::ffi::c_ulong {
    let x = x11_dl::xlib::Xlib::open().expect("libX11 loads");
    // SAFETY: a display opened, read and closed here on one thread
    unsafe {
        let display = (x.XOpenDisplay)(std::ptr::null());
        assert!(!display.is_null(), "no X display; set DISPLAY");
        let visual = (x.XDefaultVisual)(display, (x.XDefaultScreen)(display));
        let id = (x.XVisualIDFromVisual)(visual);
        (x.XCloseDisplay)(display);
        id
    }
}

/// Returns the window's whole client area in the units a child browser is
/// placed in (points on macOS, pixels on Windows and X11).
fn client_bounds(window: &Window) -> BrowserBounds {
    #[cfg(target_os = "macos")]
    let (width, height) = window.size();
    #[cfg(not(target_os = "macos"))]
    let (width, height) = window.size_in_pixels();
    // A window's size fits an i32
    embedding::filling(width as i32, height as i32)
}

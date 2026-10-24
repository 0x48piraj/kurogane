//! Chromium as a child browser in a window GTK 3 makes and a main loop GTK runs.
//!
//! GTK runs its own glib main loop with `gtk::main`. Kurogane's pump drains
//! glib's default main context too, the one GTK's loop runs. A one-shot glib
//! timeout pumps Chromium at the earliest deadline it asked for and at least
//! every 33 ms. The scheduler reaches the loop from any thread through
//! `MainContext::invoke`.
//!
//! GTK runs on X11, where Chromium needs its parent. The browser fills the
//! window's own X11 window.
//!
//! GTK keeps the keyboard focus on its own window. The host gives it to the
//! browser whenever the window gains it.
//!
//! Closing the window closes the browser first. The loop keeps pumping until
//! should_shutdown() turns true, then hides the window, shuts Kurogane down and
//! leaves GTK's main loop.

#[cfg(target_os = "linux")]
mod host {
    use std::cell::{OnceCell, RefCell};
    use std::time::Instant;

    use gdkx11::X11Window;
    use gtk::glib::{self, MainContext, Propagation, SourceId};
    use gtk::prelude::*;
    use kurogane::raw_window_handle::{
        HandleError, HasWindowHandle, RawWindowHandle, WindowHandle, XlibWindowHandle,
    };
    use kurogane::{AppInstance, BrowserBounds, BrowserHandle};
    use kurogane_suite::embedding::{self, PumpSchedule};

    /// State glib's callbacks share on the main thread.
    struct Host {
        runtime: AppInstance,
        schedule: PumpSchedule,
        window: gtk::Window,
        browser: OnceCell<BrowserHandle>,
        /// Armed timeout and when it pumps.
        timer: RefCell<Option<(SourceId, Instant)>>,
    }

    thread_local! {
        /// The host once GTK's loop runs. glib can run the scheduler's callback
        /// on the scheduler's own thread, where this stays empty.
        static HOST: OnceCell<Host> = const { OnceCell::new() };
    }

    /// Runs `f` with the host, on the main thread once GTK's loop runs.
    fn with_host(f: impl FnOnce(&Host)) {
        HOST.with(|host| {
            if let Some(host) = host.get() {
                f(host);
            }
        });
    }

    impl Host {
        /// Arms the timeout for an earlier deadline Chromium asked for.
        fn rearm(&self) {
            // Kurogane's pump drains glib's default context until nothing is
            // ready. A timeout armed during it would be ready at once; the pump
            // arms the next one once it returns
            if self.schedule.is_pumping() {
                return;
            }
            let next = self.schedule.next();
            let armed = self.timer.borrow().as_ref().map(|&(_, at)| at);
            if armed.is_none_or(|at| next < at) {
                self.arm(next);
            }
        }

        /// Replaces the armed timeout with one that pumps at `at`.
        fn arm(&self, at: Instant) {
            if let Some((timer, _)) = self.timer.take() {
                timer.remove();
            }
            let timer =
                glib::timeout_add_local_once(at.saturating_duration_since(Instant::now()), || {
                    with_host(|host| {
                        // The source is gone once it has fired
                        host.timer.take();
                        host.pump();
                    })
                });
            *self.timer.borrow_mut() = Some((timer, at));
        }

        /// Pumps Chromium, then arms the next timeout or leaves GTK's loop once
        /// the browser has closed.
        fn pump(&self) {
            if !self.schedule.pump(&self.runtime) {
                return;
            }
            if self.runtime.should_shutdown() {
                // The browser is gone, whatever closed it; the window goes first
                self.window.hide();
                self.runtime.shutdown();
                gtk::main_quit();
                return;
            }
            self.arm(self.schedule.next());
        }

        /// Places the browser over the window's whole area.
        fn fill(&self) {
            if let Some(browser) = self.browser.get() {
                browser.set_bounds(client_bounds(&self.window));
            }
        }

        /// Tells the browser its window moved. The browser learns of the
        /// window's place only from this; its popups and the page's screen
        /// position follow it.
        fn moved(&self) {
            if let Some(browser) = self.browser.get() {
                browser.notify_move_or_resize_started();
            }
        }
    }

    /// X11 window as raw-window-handle hands it to Kurogane.
    struct Parent(std::ffi::c_ulong);

    impl HasWindowHandle for Parent {
        fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
            let raw = RawWindowHandle::Xlib(XlibWindowHandle::new(self.0));
            // SAFETY: GTK keeps the window until the loop ends, after its
            // browser has closed
            Ok(unsafe { WindowHandle::borrow_raw(raw) })
        }
    }

    /// Returns the window's whole area in the device pixels X11 places a child
    /// window in.
    fn client_bounds(window: &gtk::Window) -> BrowserBounds {
        // The default size until the window is mapped, the configured one
        // after; GTK lays the window out only once its loop runs
        let (width, height) = window.size();
        let scale = window.scale_factor();
        embedding::filling(width * scale, height * scale)
    }

    /// Returns the X screen's default visual as GDK knows it.
    fn default_visual(window: &gtk::Window) -> Option<gdkx11::X11Visual> {
        let screen = WidgetExt::screen(window)?
            .downcast::<gdkx11::X11Screen>()
            .ok()?;
        let x = x11_dl::xlib::Xlib::open().ok()?;
        // SAFETY: a display opened, read and closed here on one thread
        let id = unsafe {
            let display = (x.XOpenDisplay)(std::ptr::null());
            if display.is_null() {
                return None;
            }
            let visual = (x.XDefaultVisual)(display, screen.screen_number());
            let id = (x.XVisualIDFromVisual)(visual);
            (x.XCloseDisplay)(display);
            id
        };
        screen.lookup_visual(id)
    }

    /// Runs the GTK host until its window has closed.
    pub fn run() {
        let (runtime, schedule) = embedding::start(|| {
            // Runs the callback on the thread that runs the default context,
            // GTK's main loop
            MainContext::default().invoke(|| with_host(Host::rearm));
        })
        .expect("Kurogane failed to start");

        // CEF parents a child browser to an X11 window on Linux, under
        // XWayland in a Wayland session
        gtk::gdk::set_allowed_backends("x11");
        gtk::init().expect("GTK failed to start");

        let window = gtk::Window::new(gtk::WindowType::Toplevel);
        window.set_title("Kurogane in GTK 3");
        window.set_default_size(1000, 700);
        window.connect_delete_event(|_, _| {
            // Close the browser first; the window stays until it has
            with_host(|host| host.runtime.handle().close_all_browsers(true));
            Propagation::Stop
        });
        window.connect_configure_event(|_, _| {
            // A new size or place. Chromium placed the browser once; keep it
            // filling the window and tell it of the move
            with_host(|host| {
                host.fill();
                host.moved();
            });
            false
        });
        window.connect_focus_in_event(|_, _| {
            // GTK keeps the keyboard on its own window; hand it on
            with_host(|host| {
                if let Some(browser) = host.browser.get() {
                    browser.focus();
                }
            });
            Propagation::Proceed
        });
        // Chromium's window takes the X screen's default visual and fails under
        // a parent of another one. GDK gives windows a GL visual of its own,
        // its system visual included, unless told
        if let Some(visual) = default_visual(&window) {
            window.set_visual(Some(visual.upcast_ref()));
        }
        window.show_all();

        let xid = window
            .window()
            .and_then(|gdk| gdk.downcast::<X11Window>().ok())
            .map(|x11| x11.xid())
            .expect("GTK runs on X11");

        let host = Host {
            runtime,
            schedule,
            window: window.clone(),
            browser: OnceCell::new(),
            timer: RefCell::new(None),
        };
        match host.runtime.create_child_browser(
            &Parent(xid),
            client_bounds(&window),
            &embedding::page("GTK 3"),
        ) {
            Ok(browser) => {
                // The window may have gained the focus before its browser
                // existed
                if window.has_toplevel_focus() {
                    browser.focus();
                }
                let _ = host.browser.set(browser);
            }
            Err(e) => eprintln!("the browser could not be created:\n{e}"),
        }

        // The first pump comes at once
        HOST.with(|cell| cell.get_or_init(|| host).arm(Instant::now()));

        // Runs until the pump leaves it
        gtk::main();
    }
}

fn main() {
    kurogane_suite::logging();

    #[cfg(target_os = "linux")]
    host::run();
    #[cfg(not(target_os = "linux"))]
    eprintln!("the GTK 3 host runs on Linux only");
}

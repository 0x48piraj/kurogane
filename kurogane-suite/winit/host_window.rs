//! Winit + Kurogane: Native embedded browser integration
//!
//! The host application creates and owns the native window.
//! Kurogane is attached as a child browser.
//!
//! This gives complete control over the window hierarchy,
//! layout, resize handling and application lifecycle.
//!
//! Chromium is pumped as in views_scheduler.rs: at the earliest
//! deadline it asked for, and at least every 33 ms.
//!
//! Browser shutdown is asynchronous.
//! After requesting browser closure the host must continue
//! pumping Chromium until on_before_close has completed and all
//! browser instances have been destroyed: until should_shutdown().
//! The host's own close is not the only way there; on macOS, Quit
//! (Kurogane's menu, or the Dock's) closes every browser too.
//!
//! On macOS the event loop keeps Kurogane's App, Edit and Window
//! menus (with_default_menu(false)): winit's own has no Edit menu,
//! so Cmd+C, Cmd+V and Cmd+A would reach no web view.

use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use kurogane::{App, BrowserBounds, PumpRequest};

use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::Window;

/// The longest the loop waits between pumps: cefclient's kMaxTimerDelay.
/// CEF does not promise to ask again after every pump.
const MAX_PUMP_DELAY: Duration = Duration::from_millis(1000 / 30);

struct EmbeddedDriver {
    handle: kurogane::AppInstance,
    window: Option<Window>,
    browser: Option<kurogane::BrowserHandle>,
    /// The earliest deadline CEF asked for, and never later than
    /// MAX_PUMP_DELAY after the last pump
    next_pump: Instant,
}

impl ApplicationHandler<Instant> for EmbeddedDriver {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let window = event_loop
            .create_window(Window::default_attributes().with_title("Kurogane Embedded"))
            .unwrap();

        // Map the child browser layout bounds 1:1 with the parent container
        let bounds = client_bounds(&window);
        match self
            .handle
            .create_child_browser(&window, bounds, "app://app/index.html")
        {
            Ok(browser) => self.browser = Some(browser),
            Err(e) => eprintln!("the browser could not be created:\n{e}"),
        }

        self.window = Some(window);
    }

    fn user_event(&mut self, _: &ActiveEventLoop, deadline: Instant) {
        // Keep the earliest: pumping early is harmless, pumping late stalls
        // Chromium
        self.next_pump = self.next_pump.min(deadline);
    }

    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _id: winit::window::WindowId,
        event: WindowEvent,
    ) {
        println!("window event: {:?}", event);
        match event {
            WindowEvent::CloseRequested => {
                // Begin asynchronous browser shutdown; the window stays until
                // every browser has closed
                self.handle.handle().close_all_browsers(true);
            }
            WindowEvent::Resized(_) => {
                // Chromium placed the browser once; keep it filling the window
                if let (Some(browser), Some(window)) = (&self.browser, &self.window) {
                    browser.set_bounds(client_bounds(window));
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Drive pending Chromium work, including browser shutdown
        let now = Instant::now();
        if now >= self.next_pump {
            // Until CEF asks for an earlier pump
            self.next_pump = now + MAX_PUMP_DELAY;
            self.handle.pump();
        }

        if self.handle.should_shutdown() {
            // Shutdown after the final browser has been destroyed, whatever
            // closed it
            self.window = None;
            self.handle.shutdown();
            event_loop.exit();
            return;
        }

        event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_pump));
    }
}

/// The window's whole client area, in the units a child browser is placed in:
/// points on macOS, pixels on Windows and X11
fn client_bounds(window: &Window) -> BrowserBounds {
    let size = window.inner_size();
    #[cfg(target_os = "macos")]
    let size = size.to_logical::<u32>(window.scale_factor());
    BrowserBounds {
        x: 0,
        y: 0,
        width: size.width as i32,
        height: size.height as i32,
    }
}

fn main() {
    kurogane_suite::logging();
    // Kurogane starts before winit: on macOS it installs the NSApplication
    // subclass CEF needs, which must happen before winit creates the
    // application. The scheduler wakes the event loop once it exists
    let wake: Arc<OnceLock<EventLoopProxy<Instant>>> = Arc::default();

    let handle = App::new("winit/frontend")
        .scheduler({
            let wake = wake.clone();
            move |request: PumpRequest| {
                // Marshal Chromium's deadline onto the event loop thread
                if let Some(proxy) = wake.get() {
                    let _ = proxy.send_event(request.deadline(Instant::now()));
                }
            }
        })
        .start_embedded()
        .expect("Kurogane failed to initialize");

    // CEF parents a child browser to an X11 window on Linux, so the loop
    // asks winit for X11, under XWayland in a Wayland session
    #[cfg(target_os = "linux")]
    let event_loop = {
        use winit::platform::x11::EventLoopBuilderExtX11;
        EventLoop::<Instant>::with_user_event().with_x11().build()
    };
    // Keep Kurogane's application menu instead of winit's default menu
    #[cfg(target_os = "macos")]
    let event_loop = {
        use winit::platform::macos::EventLoopBuilderExtMacOS;
        EventLoop::<Instant>::with_user_event()
            .with_default_menu(false)
            .build()
    };
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let event_loop = EventLoop::<Instant>::with_user_event().build();
    let event_loop = event_loop.unwrap();
    let _ = wake.set(event_loop.create_proxy());

    let mut app = EmbeddedDriver {
        handle,
        window: None,
        browser: None,
        // At once: requests made before the proxy was set went nowhere
        next_pump: Instant::now(),
    };

    event_loop.run_app(&mut app).unwrap();
}

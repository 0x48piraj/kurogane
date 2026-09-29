//! Winit + Kurogane: Native embedded browser integration
//!
//! The host application creates and owns the native window.
//! Kurogane is attached as a child browser.
//!
//! This gives complete control over the window hierarchy,
//! layout, resize handling and application lifecycle.
//!
//! Browser shutdown is asynchronous.
//! After requesting browser closure the host must continue
//! pumping Chromium until on_before_close has completed and all
//! browser instances have been destroyed.

use std::sync::{Arc, OnceLock};

use kurogane::{App, BrowserBounds, PumpRequest};

use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::Window;

struct EmbeddedDriver {
    handle: kurogane::AppInstance,
    window: Option<Window>,
    browser: Option<kurogane::BrowserHandle>,
    closing: bool,
}

impl ApplicationHandler for EmbeddedDriver {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let window = event_loop
            .create_window(Window::default_attributes().with_title("Kurogane Embedded"))
            .unwrap();

        let hwnd = native_handle(&window);

        // Map the child browser layout bounds 1:1 with the parent container
        let bounds = client_bounds(&window);
        self.browser = self
            .handle
            .create_child_browser(hwnd, bounds, "app://app/index.html");

        self.window = Some(window);
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
                self.closing = true;

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
        self.handle.pump();

        if self.closing && self.handle.handle().browser_count() == 0 {
            // Shutdown after the final browser has been destroyed
            self.window = None;
            self.handle.shutdown();
            event_loop.exit();
        }
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

/// Helper function to extract a platform-native window handle for browser embedding
fn native_handle(window: &Window) -> *mut std::ffi::c_void {
    let handle = window.window_handle().unwrap();
    match handle.as_raw() {
        #[cfg(target_os = "windows")]
        RawWindowHandle::Win32(h) => h.hwnd.get() as *mut _,
        #[cfg(target_os = "macos")]
        RawWindowHandle::AppKit(h) => h.ns_view.as_ptr(),
        #[cfg(target_os = "linux")]
        RawWindowHandle::Xlib(h) => h.window as usize as *mut _,
        #[cfg(target_os = "linux")]
        RawWindowHandle::Wayland(h) => h.surface.as_ptr(),
        _ => panic!("unsupported platform"),
    }
}

fn main() {
    // Kurogane starts before winit: on macOS it installs the NSApplication
    // subclass CEF needs, which must happen before winit creates the
    // application. The scheduler wakes the event loop once it exists
    let wake: Arc<OnceLock<EventLoopProxy<()>>> = Arc::default();

    let handle = App::new("winit/frontend")
        .scheduler({
            let wake = wake.clone();
            move |_request: PumpRequest| {
                // Marshal Chromium wake requests onto the event loop thread
                if let Some(proxy) = wake.get() {
                    let _ = proxy.send_event(());
                }
            }
        })
        .start_embedded()
        .expect("Kurogane failed to initialize");

    let event_loop = EventLoop::new().unwrap();
    let _ = wake.set(event_loop.create_proxy());
    event_loop.set_control_flow(ControlFlow::Wait);

    let mut app = EmbeddedDriver {
        handle,
        window: None,
        browser: None,
        closing: false,
    };

    event_loop.run_app(&mut app).unwrap();
}

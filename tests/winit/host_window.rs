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
//! pumping CEF until on_before_close has completed and all
//! browser instances have been destroyed.

use kurogane::{App, BrowserBounds, PumpRequest};

use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::Window;

struct EmbeddedDriver {
    handle: kurogane::RuntimeHandle,
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
            .create_window(
                Window::default_attributes()
                    .with_title("Kurogane Embedded"),
            )
            .unwrap();

        let size = window.inner_size();
        let hwnd = native_handle(&window);

        self.browser = self.handle.create_child_browser(
            hwnd,
            BrowserBounds {
                x: 0,
                y: 0,
                width: size.width as i32,
                height: size.height as i32,
            },
            "app://app/index.html",
        );

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
                self.handle.close_all_browsers(true);

                if let Some(window) = self.window.take() {
                    drop(window);
                }
            }
            WindowEvent::Resized(_) => {
                if let Some(browser) = &self.browser {
                    browser.notify_resized();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.handle.pump();

        if self.closing && self.handle.browser_count() == 0 {
            // All browsers gone
            self.handle.shutdown();
            event_loop.exit();
        }
    }
}

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
    let event_loop = EventLoop::new().unwrap();
    let proxy = event_loop.create_proxy();

    let handle = App::new("dom")
        .scheduler(move |request: PumpRequest| {
            let _ = proxy.send_event(());
        })
        .start_embedded()
        .unwrap();

    event_loop.set_control_flow(ControlFlow::Wait);

    let mut app = EmbeddedDriver {
        handle,
        window: None,
        browser: None,
        closing: false,
    };

    event_loop.run_app(&mut app).unwrap();
}

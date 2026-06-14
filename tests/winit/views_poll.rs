//! Winit + Kurogane: Views mode (polling)
//!
//! Chromium owns the native window via the Views framework.
//! The host application owns the outer winit event loop.
//!
//! This example calls Kurogane's pump() on every
//! iteration of the event loop using ControlFlow::Poll.
//!
//! This approach is generally useful for experimentation
//! and debugging, but is not the most efficient production
//! integration.

use kurogane::App;
use winit::application::ApplicationHandler;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};

struct ViewsDriver {
    handle: kurogane::RuntimeHandle,
}

impl ApplicationHandler for ViewsDriver {
    fn resumed(
        &mut self,
        _: &ActiveEventLoop,
    ) {}

    fn window_event(
        &mut self,
        _: &ActiveEventLoop,
        _: winit::window::WindowId,
        _: winit::event::WindowEvent,
    ) {}

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Pump Chromium on every event loop iteration
        self.handle.pump();

        // Exit after all Chromium Views windows have been closed
        if self.handle.should_shutdown() {
            event_loop.exit();
        }
    }
}

fn main() {
    let handle = App::url("https://example.com")
        .start()
        .unwrap();

    let event_loop = EventLoop::new().unwrap();

    // Continuously iterate the event loop
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = ViewsDriver { handle };
    event_loop.run_app(&mut app).unwrap();
}

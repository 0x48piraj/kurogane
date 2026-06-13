//! Winit + Kurogane: Views mode (fixed interval pumping)
//!
//! CEF owns the native window via the Views framework.
//! The host application owns the outer winit event loop.
//!
//! This example pumps Chromium at a fixed interval
//! (~60Hz using a 16ms timer).
//!
//! This approach is simple, predictable and does not
//! require scheduler callbacks.

use std::time::{Duration, Instant};

use kurogane::{App, RuntimeHandle};

use winit::application::ApplicationHandler;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};

const PUMP_INTERVAL: Duration = Duration::from_millis(16);

struct ViewsDriver {
    handle: RuntimeHandle,
    next_pump: Instant,
}

impl ViewsDriver {
    fn new(handle: RuntimeHandle) -> Self {
        Self {
            handle,
            next_pump: Instant::now(),
        }
    }

    fn pump(&mut self) {
        self.handle.pump();
        self.next_pump = Instant::now() + PUMP_INTERVAL;
    }
}

impl ApplicationHandler for ViewsDriver {
    fn resumed(&mut self, _event_loop: &ActiveEventLoop) {
        // CEF Views owns its own window; nothing for winit to create
    }

    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _id: winit::window::WindowId,
        _event: winit::event::WindowEvent,
    ) {
        // No winit windows to handle events for
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if Instant::now() >= self.next_pump {
            self.pump();
        }

        if self.handle.should_shutdown() {
            event_loop.exit();
            return;
        }

        event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_pump));
    }
}

fn main() {
    let handle = App::url("https://example.com")
        .start()
        .expect("failed to start kurogane runtime");

    let event_loop = EventLoop::new().expect("failed to create event loop");
    event_loop.set_control_flow(ControlFlow::WaitUntil(
        Instant::now() + PUMP_INTERVAL,
    ));

    let mut driver = ViewsDriver::new(handle);
    event_loop.run_app(&mut driver).expect("event loop failed");
}

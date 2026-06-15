//! Winit + Kurogane: Views mode (fixed interval pumping)
//!
//! Chromium owns the native window via the Views framework.
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
        // Window creation is handled by Chromium Views
    }

    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _id: winit::window::WindowId,
        _event: winit::event::WindowEvent,
    ) {
        // Chromium Views owns all native windows
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Maintain a fixed pumping cadence independent of OS event frequency
        if Instant::now() >= self.next_pump {
            self.pump();
        }

        if self.handle.should_shutdown() {
            event_loop.exit();
            return;
        }

        // Continue waking at fixed intervals even when Chromium is idle
        event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_pump));
    }
}

fn main() {
    let handle = App::url("https://example.com")
        .start()
        .expect("Kurogane failed to initialize");

    let event_loop = EventLoop::new().expect("failed to create event loop");
    
    // Initialize the fixed-interval pumping schedule
    event_loop.set_control_flow(ControlFlow::WaitUntil(
        Instant::now() + PUMP_INTERVAL,
    ));

    let mut driver = ViewsDriver::new(handle);
    event_loop.run_app(&mut driver).expect("event loop failed");
}

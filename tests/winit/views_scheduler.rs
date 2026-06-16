//! Winit + Kurogane: Views mode (scheduler driven)
//!
//! Chromium owns the native window via the Views framework.
//! The host application owns the outer winit event loop.
//!
//! Unlike polling-based integrations, this example uses
//! App::scheduler() to wake the event loop only when
//! Chromium requests additional work.
//!
//! This minimizes unnecessary wakeups and is the
//! preferred event-driven integration pattern.

use kurogane::{App, PumpRequest, Runtime};

use winit::application::ApplicationHandler;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};

struct ViewsDriver {
    handle: Runtime,
}

impl ApplicationHandler for ViewsDriver {
    fn resumed(&mut self, _: &ActiveEventLoop) {}

    fn window_event(
        &mut self,
        _: &ActiveEventLoop,
        _: winit::window::WindowId,
        _: winit::event::WindowEvent,
    ) {
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Invoked after an OS event or scheduler-triggered wakeup.
        // Pump pending Chromium work before the event loop sleeps again.
        self.handle.pump();

        if self.handle.should_shutdown() {
            event_loop.exit();
        }
    }
}

fn main() {
    // Enable user events so Chromium's scheduler callback can wake the event loop
    let event_loop = EventLoop::<()>::with_user_event()
        .build()
        .unwrap();

    let proxy = event_loop.create_proxy();

    let handle = App::url("https://example.com")
        .scheduler(move |_request: PumpRequest| {
            // Executed on Chromium's UI thread
            // EventLoopProxy provides a thread-safe wakeup mechanism
            // The request payload is ignored because any wakeup triggers a pump
            let _ = proxy.send_event(());
        })
        .start()
        .expect("Kurogane failed to initialize");

    // Sleep until an OS event or scheduler wakeup is received
    event_loop.set_control_flow(ControlFlow::Wait);

    let mut app = ViewsDriver { handle };

    event_loop.run_app(&mut app).unwrap();
}

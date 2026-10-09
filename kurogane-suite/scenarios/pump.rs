//! Pumps CEF from the application's own loop, without a scheduler.
//!
//! CEF recommends its external message pump for an application that pumps
//! it (cef_app.h, CefDoMessageLoopWork); this loop runs without one and
//! sleeps a fixed 16 ms between pumps. winit/views_scheduler.rs pumps when
//! CEF asks, and an application without a loop of its own calls App::run.

use std::time::Duration;

use kurogane::App;

fn main() {
    kurogane_suite::logging();
    let kurogane = App::url("https://example.com")
        .start()
        .expect("Kurogane failed to initialize");

    let tick = Duration::from_millis(16); // optional

    while !kurogane.should_shutdown() {
        kurogane.pump();
        std::thread::sleep(tick);
    }

    kurogane.shutdown();
}

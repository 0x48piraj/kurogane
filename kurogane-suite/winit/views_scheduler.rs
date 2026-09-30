//! Winit + Kurogane: Views mode (scheduler driven)
//!
//! Chromium owns the native window via the Views framework.
//! The host application owns the outer winit event loop.
//!
//! Unlike polling-based integrations, this example uses
//! App::scheduler() so Chromium tells the event loop when
//! it next needs a pump.
//!
//! Each request becomes a deadline. The loop pumps at the
//! earliest one and never waits more than 33 ms between pumps,
//! as CEF's cefclient does.
//!
//! KUROGANE_PUMP_STATS=1 prints how often it pumps, once a second.

use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use kurogane::{App, PumpRequest};

use winit::application::ApplicationHandler;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};

/// The longest the loop waits between pumps: cefclient's kMaxTimerDelay.
/// CEF does not promise to ask again after every pump.
const MAX_PUMP_DELAY: Duration = Duration::from_millis(1000 / 30);

struct ViewsDriver {
    handle: kurogane::AppInstance,
    /// The earliest deadline CEF asked for, and never later than
    /// MAX_PUMP_DELAY after the last pump
    next_pump: Instant,
    stats: Option<PumpStats>,
}

impl ApplicationHandler<Instant> for ViewsDriver {
    fn resumed(&mut self, _: &ActiveEventLoop) {}

    fn user_event(&mut self, _: &ActiveEventLoop, deadline: Instant) {
        // Keep the earliest: pumping early is harmless, pumping late stalls
        // Chromium. Replacing it, as CEF's header describes, could push back
        // a request to pump now that has not been served yet
        self.next_pump = self.next_pump.min(deadline);
    }

    fn window_event(
        &mut self,
        _: &ActiveEventLoop,
        _: winit::window::WindowId,
        _: winit::event::WindowEvent,
    ) {
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Invoked after an OS event, a scheduler request or a deadline
        let now = Instant::now();
        if now >= self.next_pump {
            // Until CEF asks for an earlier pump
            self.next_pump = now + MAX_PUMP_DELAY;
            self.handle.pump();
            if let Some(stats) = &mut self.stats {
                stats.pumped(now);
            }
        }

        if self.handle.should_shutdown() {
            event_loop.exit();
            return;
        }

        // Sleep until an OS event, a new request or the deadline
        event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_pump));
    }
}

/// Pumps per second and the longest gap between two, for KUROGANE_PUMP_STATS
struct PumpStats {
    since: Instant,
    last: Instant,
    pumps: u32,
    longest_gap: Duration,
}

impl PumpStats {
    fn new(now: Instant) -> Self {
        Self {
            since: now,
            last: now,
            pumps: 0,
            longest_gap: Duration::ZERO,
        }
    }

    fn pumped(&mut self, now: Instant) {
        self.pumps += 1;
        self.longest_gap = self.longest_gap.max(now - self.last);
        self.last = now;

        let elapsed = now - self.since;
        if elapsed >= Duration::from_secs(1) {
            println!(
                "pump-stats: {} pumps in {} ms, longest gap {} ms",
                self.pumps,
                elapsed.as_millis(),
                self.longest_gap.as_millis()
            );
            *self = Self::new(now);
        }
    }
}

fn main() {
    // Kurogane starts before winit: on macOS it installs the NSApplication
    // subclass CEF needs, which must happen before winit creates the
    // application. The scheduler wakes the event loop once it exists
    let wake: Arc<OnceLock<EventLoopProxy<Instant>>> = Arc::default();

    let handle = App::url("https://example.com")
        .scheduler({
            let wake = wake.clone();
            move |request: PumpRequest| {
                // CEF may call this from any thread
                // EventLoopProxy hands the deadline to the event loop thread
                if let Some(proxy) = wake.get() {
                    let _ = proxy.send_event(request.deadline(Instant::now()));
                }
            }
        })
        .start()
        .expect("Kurogane failed to initialize");

    // Enable user events so the scheduler's deadlines reach the event loop
    let event_loop = EventLoop::<Instant>::with_user_event().build().unwrap();
    let _ = wake.set(event_loop.create_proxy());

    let now = Instant::now();
    let stats = std::env::var("KUROGANE_PUMP_STATS").as_deref() == Ok("1");
    let mut app = ViewsDriver {
        handle,
        // At once: requests made before the proxy was set went nowhere
        next_pump: now,
        stats: stats.then(|| PumpStats::new(now)),
    };

    event_loop.run_app(&mut app).unwrap();
}

//! Shared parts of the embedding hosts.
//!
//! Every host loads the same page, answers the same commands and pumps
//! Chromium on the same schedule. A host adds its window, its loop and a way
//! to wake that loop from any thread.

#[cfg(target_os = "macos")]
mod pump_timer;

use std::cell::Cell;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use kurogane::{App, AppHandle, AppInstance, BrowserBounds, IpcError, PumpRequest, RuntimeError};

#[cfg(target_os = "macos")]
pub use pump_timer::PumpTimer;

/// Longest wait between two pumps, cefclient's kMaxTimerDelay.
///
/// CEF does not promise to ask again after every pump.
const MAX_PUMP_DELAY: Duration = Duration::from_millis(1000 / 30);

/// Starts Kurogane embedded with the page's commands.
///
/// `wake` ends the host loop's wait. It runs on any thread each time
/// Chromium asks for a pump; the returned schedule holds when.
///
/// # Errors
///
/// As [`App::start_embedded`].
pub fn start(
    wake: impl Fn() + Send + Sync + 'static,
) -> Result<(AppInstance, PumpSchedule), RuntimeError> {
    let (request, requests) = mpsc::channel();
    let runtime = App::new("embedding/frontend")
        .command("host.ping", ping)
        .command("host.report", report)
        .scheduler(move |pump: PumpRequest| {
            // The schedule reads the deadline; the wake only ends the wait
            if request.send(pump.deadline(Instant::now())).is_ok() {
                wake();
            }
        })
        .start_embedded()?;
    Ok((runtime, PumpSchedule::new(requests)))
}

/// Returns the URL of the page for the host named `name`.
pub fn page(name: &str) -> String {
    format!("app://app/index.html?host={name}")
}

/// Returns bounds that fill a parent window of `width` by `height`.
pub fn filling(width: i32, height: i32) -> BrowserBounds {
    BrowserBounds {
        x: 0,
        y: 0,
        width,
        height,
    }
}

/// Answers the page's call with the host's name.
fn ping(host: String, _: &AppHandle) -> Result<String, IpcError> {
    Ok(format!("pong from Rust in {host}"))
}

/// Prints each load, resize, keystroke and reply the page reports.
fn report(what: String, _: &AppHandle) -> Result<(), IpcError> {
    println!("page: {what}");
    Ok(())
}

/// When Chromium next needs a pump.
///
/// The earliest deadline Chromium asked for, never later than
/// [`MAX_PUMP_DELAY`] after the last pump. A pump never runs inside another;
/// a toolkit's callbacks can run inside one. Lives on the thread that started
/// Kurogane.
pub struct PumpSchedule {
    /// Deadlines the scheduler sends from any thread.
    requests: Receiver<Instant>,
    /// Deadline of the next pump.
    next: Cell<Instant>,
    /// Whether a pump runs now.
    pumping: Cell<bool>,
}

impl PumpSchedule {
    fn new(requests: Receiver<Instant>) -> Self {
        Self {
            requests,
            // At once, for requests that came before the host's loop ran
            next: Cell::new(Instant::now()),
            pumping: Cell::new(false),
        }
    }

    /// Returns when the next pump is due.
    pub fn next(&self) -> Instant {
        // A later request never pushes back an earlier one not yet served
        let next = self.requests.try_iter().fold(self.next.get(), Instant::min);
        self.next.set(next);
        next
    }

    /// Returns how long the loop may wait before the next pump.
    pub fn wait(&self) -> Duration {
        self.next().saturating_duration_since(Instant::now())
    }

    /// Returns the wait in whole milliseconds, rounded up so the loop never
    /// wakes before the pump is due.
    pub fn wait_ms(&self) -> u32 {
        u32::try_from(self.wait().as_nanos().div_ceil(1_000_000)).unwrap_or(u32::MAX)
    }

    /// Returns whether the next pump is due.
    pub fn is_due(&self) -> bool {
        Instant::now() >= self.next()
    }

    /// Returns whether a pump runs now.
    pub fn is_pumping(&self) -> bool {
        self.pumping.get()
    }

    /// Pumps Chromium once the next pump is due.
    pub fn pump_if_due(&self, runtime: &AppInstance) {
        if self.is_due() {
            self.pump(runtime);
        }
    }

    /// Pumps Chromium unless a pump runs already. Returns whether it pumped.
    pub fn pump(&self, runtime: &AppInstance) -> bool {
        if self.pumping.replace(true) {
            return false;
        }
        // Until Chromium asks for an earlier pump
        self.next.set(Instant::now() + MAX_PUMP_DELAY);
        runtime.pump();
        self.pumping.set(false);
        true
    }
}

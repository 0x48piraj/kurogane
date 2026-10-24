//! macOS timer that pumps Chromium inside AppKit's own loops.
//!
//! AppKit runs a loop of its own while the user resizes a window or tracks a
//! menu. A host's loop waits until it ends. A timer in the run loop's common
//! modes fires inside it too, as cefclient's pump does.

use std::cell::RefCell;
use std::time::Duration;

use kurogane::AppInstance;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_foundation::{NSRunLoop, NSRunLoopCommonModes, NSTimer};

use super::PumpSchedule;

/// Closure a timer runs when it fires.
type Fire = Box<dyn Fn()>;

define_class! {
    /// Object the timer calls, which runs the closure.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "KuroganeSuitePumpTimerTarget"]
    #[ivars = Fire]
    struct Target;

    impl Target {
        /// Runs the closure when the timer fires.
        #[unsafe(method(fire:))]
        fn fire(&self, _timer: &NSTimer) {
            (self.ivars())();
        }
    }
}

/// One-shot timer armed for the next deadline after each pump, firing in every
/// mode of the main run loop, AppKit's own loops included.
///
/// Dropping it disarms it.
pub struct PumpTimer {
    target: Retained<Target>,
    /// Timer armed now.
    armed: RefCell<Option<Retained<NSTimer>>>,
}

impl PumpTimer {
    /// Makes a timer that runs `fire` each time it fires. `fire` reaches the
    /// host and calls [`pump_if_due`](Self::pump_if_due).
    pub fn new(mtm: MainThreadMarker, fire: impl Fn() + 'static) -> Self {
        let target = Target::alloc(mtm).set_ivars(Box::new(fire) as Fire);
        // SAFETY: NSObject's designated initializer on a freshly allocated
        // instance whose ivars are set
        let target = unsafe { msg_send![super(target), init] };
        Self {
            target,
            armed: RefCell::new(None),
        }
    }

    /// Pumps Chromium once a pump is due, then arms the timer for the next
    /// deadline.
    pub fn pump_if_due(&self, schedule: &PumpSchedule, runtime: &AppInstance) {
        // A pump inside another leaves the timer to the outer one
        if schedule.is_due() && schedule.pump(runtime) && !runtime.should_shutdown() {
            self.arm(schedule.wait());
        }
    }

    /// Arms the timer to fire once after `wait`, in place of an armed one.
    fn arm(&self, wait: Duration) {
        self.disarm();
        let target: &AnyObject = &self.target;
        // SAFETY: the target answers `fire:` with the argument a timer passes;
        // the common modes include those of AppKit's own loops, event tracking
        // and modal panels
        let timer = unsafe {
            let timer = NSTimer::timerWithTimeInterval_target_selector_userInfo_repeats(
                wait.as_secs_f64(),
                target,
                sel!(fire:),
                None,
                false,
            );
            NSRunLoop::currentRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes);
            timer
        };
        *self.armed.borrow_mut() = Some(timer);
    }

    /// Disarms the timer.
    pub fn disarm(&self) {
        if let Some(timer) = self.armed.take() {
            timer.invalidate();
        }
    }
}

impl Drop for PumpTimer {
    fn drop(&mut self) {
        self.disarm();
    }
}

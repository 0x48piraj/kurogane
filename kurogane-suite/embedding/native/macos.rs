//! AppKit host, an `NSWindow` and a loop over the application's events.
//!
//! The loop takes each event with `nextEventMatchingMask:untilDate:` and sends
//! it on. The scheduler posts an application-defined event from any thread to
//! end the loop's wait; AppKit allows `postEvent:atStart:` off the main thread.
//!
//! AppKit runs a loop of its own while the user resizes the window or tracks a
//! menu. The host's loop waits until it ends. A timer in the run loop's common
//! modes, armed for the next deadline after each pump, pumps Chromium inside
//! it, as cefclient does.
//!
//! The window's delegate turns a close into a close of the browser first.
//! Chromium's view stretches with the window's content view by itself.

use std::cell::OnceCell;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::time::Duration;

use kurogane::raw_window_handle::{
    AppKitWindowHandle, HandleError, HasWindowHandle, RawWindowHandle, WindowHandle,
};
use kurogane::{AppHandle, AppInstance, BrowserBounds};
use kurogane_suite::embedding::{self, PumpSchedule, PumpTimer};
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyObject, Bool, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, class, define_class, msg_send};
use objc2_app_kit::{
    NSApp, NSApplication, NSBackingStoreType, NSEventMask, NSEventType, NSView, NSWindow,
    NSWindowDelegate, NSWindowStyleMask,
};
use objc2_foundation::{NSDate, NSDefaultRunLoopMode, NSPoint, NSRect, NSSize, NSString};

define_class! {
    /// Window delegate that closes the browser before the window.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = AppHandle]
    struct HostDelegate;

    unsafe impl NSObjectProtocol for HostDelegate {}

    unsafe impl NSWindowDelegate for HostDelegate {
        /// Starts closing the browser and keeps the window open until it has.
        #[unsafe(method(windowShouldClose:))]
        fn window_should_close(&self, _sender: &NSWindow) -> Bool {
            self.ivars().close_all_browsers(true);
            Bool::NO
        }
    }
}

impl HostDelegate {
    fn new(mtm: MainThreadMarker, app: AppHandle) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(app);
        // SAFETY: NSObject's designated initializer on a freshly allocated
        // instance whose ivars are set
        unsafe { msg_send![super(this), init] }
    }
}

/// What the loop and the pump timer share on the main thread.
struct Host {
    runtime: AppInstance,
    schedule: PumpSchedule,
    /// Timer that pumps inside AppKit's own loops.
    timer: PumpTimer,
}

thread_local! {
    /// The host once the loop runs.
    static HOST: OnceCell<Host> = const { OnceCell::new() };
}

/// Runs `f` with the host once the loop runs.
fn with_host(f: impl FnOnce(&Host)) {
    HOST.with(|host| {
        if let Some(host) = host.get() {
            f(host);
        }
    });
}

impl Host {
    /// Runs the loop until the browser has closed, then closes the window and
    /// shuts Kurogane down.
    fn run(&self, app: &NSApplication, window: &NSWindow) {
        while !self.runtime.should_shutdown() {
            dispatch_events(app, self.schedule.wait());
            self.pump_if_due();
        }

        // The browser is gone, whatever closed it; the window goes first
        self.timer.disarm();
        window.setDelegate(None);
        window.close();
        self.runtime.shutdown();
    }

    /// Pumps Chromium once a pump is due, then arms the timer for the next
    /// deadline.
    fn pump_if_due(&self) {
        self.timer.pump_if_due(&self.schedule, &self.runtime);
    }
}

/// AppKit view as raw-window-handle hands it to Kurogane.
struct Parent(NonNull<c_void>);

impl HasWindowHandle for Parent {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let raw = RawWindowHandle::AppKit(AppKitWindowHandle::new(self.0));
        // SAFETY: the view lives until the loop ends, after its browser has
        // closed
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}

/// Runs the AppKit host until its window has closed.
pub fn run() {
    let (runtime, schedule) = embedding::start(wake).expect("Kurogane failed to start");

    let mtm = MainThreadMarker::new().expect("the host runs on the main thread");
    let app = NSApp(mtm);
    let window = create_window(mtm);
    let delegate = HostDelegate::new(mtm, runtime.handle().clone());
    window.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    window.makeKeyAndOrderFront(None);

    let view = window.contentView().expect("a window has a content view");
    if let Err(e) = runtime.create_child_browser(
        &Parent(NonNull::from(&*view).cast()),
        bounds_of(&view),
        &embedding::page("AppKit"),
    ) {
        eprintln!("the browser could not be created:\n{e}");
    }

    let host = Host {
        runtime,
        schedule,
        timer: PumpTimer::new(mtm, || with_host(Host::pump_if_due)),
    };
    HOST.with(|cell| cell.get_or_init(|| host).run(&app, &window));
}

/// Makes the host window, centered on its screen.
fn create_window(mtm: MainThreadMarker) -> Retained<NSWindow> {
    let content = NSRect::new(NSPoint::new(200.0, 200.0), NSSize::new(1000.0, 700.0));
    let style = NSWindowStyleMask::Titled
        | NSWindowStyleMask::Closable
        | NSWindowStyleMask::Miniaturizable
        | NSWindowStyleMask::Resizable;
    // SAFETY: a new window with a valid frame and style on the main thread
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            content,
            style,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    // SAFETY: Retained owns the window; a close must not release it too
    unsafe { window.setReleasedWhenClosed(false) };
    window.setTitle(&NSString::from_str("Kurogane in AppKit"));
    window.center();
    window
}

/// Sends on each event that arrives within `wait`, then every event already
/// queued.
fn dispatch_events(app: &NSApplication, wait: Duration) {
    autoreleasepool(|_| {
        // SAFETY: a constant Foundation exports, read on the main thread
        let mode = unsafe { NSDefaultRunLoopMode };
        let until = NSDate::dateWithTimeIntervalSinceNow(wait.as_secs_f64());
        let mut event = app.nextEventMatchingMask_untilDate_inMode_dequeue(
            NSEventMask::Any,
            Some(&until),
            mode,
            true,
        );
        // The rest without waiting
        let now = NSDate::distantPast();
        while let Some(next) = event {
            app.sendEvent(&next);
            event = app.nextEventMatchingMask_untilDate_inMode_dequeue(
                NSEventMask::Any,
                Some(&now),
                mode,
                true,
            );
        }
    });
}

/// Returns the view's bounds in points, the units a child browser is placed
/// in on macOS.
fn bounds_of(view: &NSView) -> BrowserBounds {
    let size = view.frame().size;
    // Whole points; a view's size fits an i32
    embedding::filling(size.width as i32, size.height as i32)
}

/// Posts an application-defined event that ends the loop's wait.
fn wake() {
    autoreleasepool(|_| {
        // SAFETY: the application exists once Kurogane has started; AppKit
        // allows creating an event and postEvent:atStart: on any thread
        unsafe {
            let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
            let event: *mut AnyObject = msg_send![
                class!(NSEvent),
                otherEventWithType: NSEventType::ApplicationDefined,
                location: NSPoint::new(0.0, 0.0),
                modifierFlags: 0usize,
                timestamp: 0.0f64,
                windowNumber: 0isize,
                context: std::ptr::null_mut::<AnyObject>(),
                subtype: 0i16,
                data1: 0isize,
                data2: 0isize
            ];
            if !event.is_null() {
                let _: () = msg_send![app, postEvent: event, atStart: false];
            }
        }
    });
}

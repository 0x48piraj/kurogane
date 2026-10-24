//! Chromium as a child browser in a window the operating system's own API makes.
//!
//! No toolkit sits between the application and the operating system. Win32
//! makes the window on Windows, AppKit on macOS and Xlib on Linux. Each hands
//! its native handle to Kurogane through raw-window-handle.
//!
//! Chromium is pumped at the earliest deadline it asked for and at least every
//! 33 ms. The scheduler wakes the loop from any thread.
//!
//! Closing the window closes the browser first. The loop keeps pumping until
//! should_shutdown() turns true, then destroys the window and shuts Kurogane
//! down.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;
#[cfg(target_os = "linux")]
mod x11;

fn main() {
    kurogane_suite::logging();

    #[cfg(windows)]
    windows::run();
    #[cfg(target_os = "macos")]
    macos::run();
    #[cfg(target_os = "linux")]
    x11::run();
}

//! Win32 host, a window from `CreateWindowExW` and a message loop of its own.
//!
//! The scheduler posts a message to the window from any thread. The window
//! pumps Chromium once a pump is due and otherwise arms a timer for the next
//! deadline, as cefclient does. Every modal loop Windows runs, for a move, a
//! resize or a menu, dispatches the window's messages too. Chromium keeps
//! pumping inside it.

use std::cell::OnceCell;
use std::num::NonZeroIsize;
use std::ptr::{null, null_mut};
use std::sync::{Arc, OnceLock};

use kurogane::raw_window_handle::{
    HandleError, HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle,
};
use kurogane::{AppInstance, BrowserBounds, BrowserHandle};
use kurogane_suite::embedding::{self, PumpSchedule};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWLP_USERDATA,
    GetClientRect, GetMessageW, GetWindowLongPtrW, IDC_ARROW, LoadCursorW, MSG, PostMessageW,
    PostQuitMessage, RegisterClassW, SW_SHOW, SetTimer, SetWindowLongPtrW, ShowWindow,
    TranslateMessage, USER_TIMER_MINIMUM, WM_APP, WM_CLOSE, WM_DESTROY, WM_ENTERSIZEMOVE, WM_MOVE,
    WM_SIZE, WM_TIMER, WNDCLASSW, WS_CLIPCHILDREN, WS_OVERLAPPEDWINDOW,
};

/// Message the scheduler posts to the window.
const WM_PUMP: u32 = WM_APP;

/// Timer that pumps at the next deadline.
const PUMP_TIMER: usize = 1;

/// State the window procedure shares on the UI thread.
struct Host {
    runtime: AppInstance,
    schedule: PumpSchedule,
    browser: OnceCell<BrowserHandle>,
}

impl Host {
    /// Pumps Chromium once a pump is due, else arms the timer for the next
    /// deadline.
    fn serve(&self, window: HWND) {
        if self.schedule.is_due() {
            self.pump(window);
        } else {
            self.arm(window);
        }
    }

    /// Pumps Chromium, then destroys the window once the browser has closed
    /// or arms the timer.
    fn pump(&self, window: HWND) {
        // A pump inside another leaves the timer to the outer one
        if !self.schedule.pump(&self.runtime) {
            return;
        }
        if self.runtime.should_shutdown() {
            // The browser is gone, whatever closed it; the window goes first
            // SAFETY: window is this thread's and still exists
            unsafe { DestroyWindow(window) };
        } else {
            self.arm(window);
        }
    }

    /// Arms the timer for the next deadline, in place of an armed one.
    fn arm(&self, window: HWND) {
        let wait = self.schedule.wait_ms().max(USER_TIMER_MINIMUM);
        // SAFETY: window is this thread's; the timer has no callback
        unsafe { SetTimer(window, PUMP_TIMER, wait, None) };
    }
}

/// Win32 window as raw-window-handle hands it to Kurogane.
struct Parent(NonZeroIsize);

impl HasWindowHandle for Parent {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let raw = RawWindowHandle::Win32(Win32WindowHandle::new(self.0));
        // SAFETY: the window lives until the loop ends, after its browser has
        // closed
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}

/// Runs the Win32 host until its window has closed.
pub fn run() {
    // The window the scheduler posts to, once it exists. A window handle is
    // not Send; its value is
    let target: Arc<OnceLock<isize>> = Arc::default();
    let (runtime, schedule) = embedding::start({
        let target = Arc::clone(&target);
        move || {
            if let Some(&window) = target.get() {
                // SAFETY: any thread may post to a window; a post to a window
                // destroyed since fails
                unsafe { PostMessageW(window as HWND, WM_PUMP, 0, 0) };
            }
        }
    })
    .expect("Kurogane failed to start");

    let host = Host {
        runtime,
        schedule,
        browser: OnceCell::new(),
    };
    let window = create_window(&host);
    let _ = target.set(window as isize);

    let parent = Parent(NonZeroIsize::new(window as isize).expect("CreateWindowExW made a window"));
    match host.runtime.create_child_browser(
        &parent,
        client_bounds(window),
        &embedding::page("Win32"),
    ) {
        Ok(browser) => {
            let _ = host.browser.set(browser);
        }
        Err(e) => eprintln!("the browser could not be created:\n{e}"),
    }

    // At once, for requests made before the window existed
    host.serve(window);

    // SAFETY: MSG is plain data that GetMessageW fills in
    let mut message: MSG = unsafe { std::mem::zeroed() };
    // Until the window's destruction posts WM_QUIT
    // SAFETY: message is a valid MSG for every call
    while unsafe { GetMessageW(&mut message, null_mut(), 0, 0) } > 0 {
        // SAFETY: message was just filled in by GetMessageW
        unsafe {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    host.runtime.shutdown();
}

/// Registers the window class and opens the host window for `host`.
fn create_window(host: &Host) -> HWND {
    let class = wide("KuroganeNativeHost");
    let title = wide("Kurogane in Win32");
    // SAFETY: the class and title strings outlive both calls; the window's
    // user data points at host, which outlives the window
    unsafe {
        let instance = GetModuleHandleW(null());
        let class_info = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            lpszClassName: class.as_ptr(),
            ..std::mem::zeroed()
        };
        RegisterClassW(&class_info);

        let window = CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            // Chromium's child window is left out of the parent's painting
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1000,
            700,
            null_mut(),
            null_mut(),
            instance,
            null(),
        );
        assert!(!window.is_null(), "CreateWindowExW made no window");
        SetWindowLongPtrW(window, GWLP_USERDATA, host as *const Host as isize);
        ShowWindow(window, SW_SHOW);
        window
    }
}

/// Answers the host window's messages.
extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: the user data is null or the Host run() keeps alive until after
    // the window is destroyed
    let host = unsafe { (GetWindowLongPtrW(window, GWLP_USERDATA) as *const Host).as_ref() };
    let Some(host) = host else {
        // SAFETY: the arguments are the ones Windows passed in
        return unsafe { DefWindowProcW(window, message, wparam, lparam) };
    };

    match message {
        WM_PUMP => {
            host.serve(window);
            0
        }
        WM_TIMER if wparam == PUMP_TIMER => {
            host.pump(window);
            0
        }
        WM_CLOSE => {
            // Close the browser first; a pump destroys the window once it has
            host.runtime.handle().close_all_browsers(true);
            0
        }
        WM_DESTROY => {
            // SAFETY: ends this thread's message loop
            unsafe { PostQuitMessage(0) };
            0
        }
        WM_SIZE => {
            // Chromium placed the browser once; keep it filling the window
            if let Some(browser) = host.browser.get() {
                browser.set_bounds(client_bounds(window));
            }
            0
        }
        WM_ENTERSIZEMOVE | WM_MOVE => {
            // The browser learns of the window's place only from this; its
            // popups and the page's screen position follow it
            if let Some(browser) = host.browser.get() {
                browser.notify_move_or_resize_started();
            }
            0
        }
        // SAFETY: the arguments are the ones Windows passed in
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

/// Returns the window's whole client area in pixels.
fn client_bounds(window: HWND) -> BrowserBounds {
    let mut rect = RECT::default();
    // SAFETY: window is a window of this thread and rect is writable
    unsafe { GetClientRect(window, &mut rect) };
    embedding::filling(rect.right - rect.left, rect.bottom - rect.top)
}

/// Returns `text` as a NUL-terminated UTF-16 string.
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

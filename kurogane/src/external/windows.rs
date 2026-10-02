//! Windows: `ShellExecuteExW` on a worker thread of its own.
//!
//! The registered handler can take its time (a DDE conversation, an
//! "Open with" prompt) and pump messages meanwhile, which must not happen on
//! CEF's UI thread. The worker initializes COM for the shell and waits for
//! the launch to finish before it ends (`SEE_MASK_NOASYNC`).

use std::ptr;

use tracing::warn;
use windows_sys::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
};
use windows_sys::Win32::UI::Shell::{
    SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, ShellExecuteExW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

pub(super) fn open(link: String) {
    let spawned = std::thread::Builder::new()
        .name("kurogane-open".into())
        .spawn(move || shell_open(&link));
    if let Err(error) = spawned {
        warn!("cannot start a thread to open the link: {error}");
    }
}

fn shell_open(link: &str) {
    let wide = |text: &str| text.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let verb = wide("open");
    let file = wide(link);

    // SAFETY: COM is initialized for this thread only and uninitialized on
    // it once the launch is done. `info` is zeroed, which is its valid empty
    // state, and its strings are NUL-terminated buffers that outlive the call
    unsafe {
        let com = CoInitializeEx(
            ptr::null(),
            (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
        );
        let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
        info.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.nShow = SW_SHOWNORMAL;
        if ShellExecuteExW(&mut info) == 0 {
            warn!(
                "the system did not open the link: {}",
                std::io::Error::last_os_error()
            );
        }
        // S_OK and S_FALSE both take a reference that must be released
        if com >= 0 {
            CoUninitialize();
        }
    }
}

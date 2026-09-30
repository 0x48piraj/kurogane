//! Linux-specific pumping.

use std::ffi::{c_int, c_void};
use std::sync::OnceLock;

/// glib's `g_main_context_iteration`.
type ContextIteration = unsafe extern "C" fn(context: *mut c_void, may_block: c_int) -> c_int;

/// Dispatches everything ready on glib's default main context, without
/// waiting.
///
/// Chromium reads its X11 and Wayland connections from glib sources on that
/// context. CEF's external message pump never iterates it, so an application
/// pumping CEF itself gets no input, window close or resize unless its pump
/// does; cefclient's Linux pump is a glib loop for the same reason. One
/// iteration is not enough: it dispatches only the sources ready at the
/// highest priority, and Chromium's come ready in bursts of hundreds.
pub(crate) fn dispatch_glib_events() {
    static ITERATION: OnceLock<Option<ContextIteration>> = OnceLock::new();
    let iteration = ITERATION.get_or_init(|| {
        // libglib, which libcef links and has loaded already
        // SAFETY: RTLD_DEFAULT searches the libraries already loaded; the
        // name is NUL-terminated.
        let symbol =
            unsafe { libc::dlsym(libc::RTLD_DEFAULT, c"g_main_context_iteration".as_ptr()) };
        // SAFETY: a non-null result is glib's g_main_context_iteration, whose
        // signature ContextIteration is.
        (!symbol.is_null())
            .then(|| unsafe { std::mem::transmute::<*mut c_void, ContextIteration>(symbol) })
    });
    if let Some(iteration) = iteration {
        // Until nothing is ready, as GTK applications drain it
        // SAFETY: a null context is the default one, which belongs to this
        // thread, CEF's UI thread; may_block 0 returns at once.
        while unsafe { iteration(std::ptr::null_mut(), 0) } != 0 {}
    }
}

//! macOS Chromium sandbox.
//!
//! Chromium sandboxes macOS helpers with seatbelt profiles. Each helper enters
//! its sandbox through `libcef_sandbox.dylib` before it loads the CEF framework.

use std::ffi::{CString, c_char, c_int, c_void};
use std::os::unix::ffi::OsStringExt;
use std::path::Path;
use std::sync::OnceLock;

use kurogane_layout::{bundle_cef_root, bundled_helper_path};

use crate::chromium_flags::ChromiumFlags;
use crate::error::RuntimeError;

/// Sandbox library, relative to the directory holding the CEF framework.
const SANDBOX_LIBRARY: &str =
    "Chromium Embedded Framework.framework/Libraries/libcef_sandbox.dylib";

/// Sandbox state held for the lifetime of a helper process.
struct HelperSandbox {
    _library: libloading::Library,
    _context: *mut c_void,
}

// SAFETY: the context is opaque and is never dereferenced here. The library
// remains loaded for the lifetime of the context.
unsafe impl Send for HelperSandbox {}

// SAFETY: the sandbox is initialized once and the stored values are never
// accessed after initialization.
unsafe impl Sync for HelperSandbox {}

/// The outcome of the helper's one attempt to enter the sandbox.
///
/// A failure is kept too: a partly initialized seatbelt cannot be rolled back,
/// so the attempt is never repeated.
static HELPER_SANDBOX: OnceLock<Result<HelperSandbox, String>> = OnceLock::new();

pub(super) fn apply_disabled(flags: &mut ChromiumFlags) {
    flags.set("no-sandbox");
}

/// Helpers enter the sandbox during initialization.
/// See [`initialize_helper`].
pub(super) fn sandbox_info() -> *mut u8 {
    std::ptr::null_mut()
}

/// Confirms the app runs from a bundle whose helpers can enter the sandbox.
pub(super) fn preflight(_cef_root: &Path) -> Result<(), RuntimeError> {
    match bundled_helper_path() {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err(RuntimeError::SandboxUnsupported {
            reason: "the Chromium sandbox on macOS requires running from a .app bundle \
                     (kurogane bundle --format app)"
                .into(),
        }),
        Err(e) => Err(RuntimeError::ExecutableUnavailable(e)),
    }
}

/// Enters the seatbelt sandbox in a helper process.
///
/// Must run before the CEF framework is loaded. Only the first call enters the
/// sandbox; later calls return its outcome.
pub(crate) fn initialize_helper() -> Result<(), RuntimeError> {
    match HELPER_SANDBOX.get_or_init(enter_sandbox) {
        Ok(_) => Ok(()),
        Err(reason) => Err(RuntimeError::SandboxUnavailable {
            reason: reason.clone(),
        }),
    }
}

/// Loads `libcef_sandbox.dylib` and calls its `cef_sandbox_initialize`.
fn enter_sandbox() -> Result<HelperSandbox, String> {
    let root = match bundle_cef_root() {
        Ok(Some(root)) => root,
        Ok(None) => return Err("CEF framework not found in the app bundle".into()),
        Err(e) => return Err(format!("cannot locate the running executable: {e}")),
    };

    let path = root.join(SANDBOX_LIBRARY);

    // SAFETY: Loading executes sandbox initializers. This is safe because it requires
    // no prior application state and must occur before CEF framework initialization.
    let library = unsafe { libloading::Library::new(&path) }
        .map_err(|e| format!("failed to load {}: {e}", path.display()))?;

    // SAFETY: ABI strictly matches `cef_sandbox_mac.h`.
    // The resolved pointer is never invoked after `library` is dropped.
    let initialize: unsafe extern "C" fn(c_int, *mut *mut c_char) -> *mut c_void =
        unsafe { library.get(b"cef_sandbox_initialize\0") }
            .map(|symbol| *symbol)
            .map_err(|e| format!("cef_sandbox_initialize not found: {e}"))?;

    // Unix arguments are C strings, so none holds an interior NUL
    let args = std::env::args_os()
        .map(|arg| CString::new(arg.into_vec()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("invalid command-line argument: {e}"))?;
    let argc = c_int::try_from(args.len()).map_err(|_| "too many command-line arguments")?;

    let mut argv: Vec<*mut c_char> = args.iter().map(|arg| arg.as_ptr().cast_mut()).collect();

    // C argv ends with a null pointer that argc does not count
    argv.push(std::ptr::null_mut());

    // SAFETY: `argv` is a valid, null-terminated array of C-string pointers
    // matching `argc`. Backing memory for both the array and strings strictly
    // outlives the FFI call.
    let context = unsafe { initialize(argc, argv.as_mut_ptr()) };

    if context.is_null() {
        return Err("cef_sandbox_initialize failed".into());
    }

    Ok(HelperSandbox {
        _library: library,
        _context: context,
    })
}

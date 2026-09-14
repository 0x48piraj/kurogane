//! macOS Chromium sandbox.
//!
//! Chromium sandboxes macOS helpers with seatbelt profiles. Each helper enters
//! its sandbox through `libcef_sandbox.dylib` before it loads the CEF framework.

use std::ffi::{CString, c_char, c_int, c_void};
use std::os::unix::ffi::OsStrExt;
use std::sync::OnceLock;

use kurogane_layout::{bundled_cef_root, bundled_helper_path};

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

// Written once during helper startup and never accessed afterwards
unsafe impl Send for HelperSandbox {}
unsafe impl Sync for HelperSandbox {}

static HELPER_SANDBOX: OnceLock<HelperSandbox> = OnceLock::new();

pub(super) fn apply_sandbox_flags(_flags: &mut ChromiumFlags) {
    // `no_sandbox=1` alone keeps helpers out of the seatbelt sandbox
}

/// Confirms the app runs from a bundle whose helpers can enter the sandbox.
pub(super) fn preflight() -> Result<(), RuntimeError> {
    match bundled_helper_path() {
        Ok(Some(_)) => Ok(()),
        _ => Err(RuntimeError::SandboxUnsupported {
            reason: "the Chromium sandbox on macOS requires running from a .app bundle \
                     (kurogane bundle --format app)"
                .into(),
        }),
    }
}

/// Enters the seatbelt sandbox in a helper process.
///
/// Must run before the CEF framework is loaded.
pub(crate) fn initialize_helper() -> Result<(), RuntimeError> {
    let unavailable = |reason: String| RuntimeError::SandboxUnavailable { reason };

    let root = bundled_cef_root()
        .ok()
        .flatten()
        .ok_or_else(|| unavailable("CEF framework not found in the app bundle".into()))?;

    let path = root.join(SANDBOX_LIBRARY);

    let library = unsafe { libloading::Library::new(&path) }
        .map_err(|e| unavailable(format!("failed to load {}: {e}", path.display())))?;

    let initialize: unsafe extern "C" fn(c_int, *mut *mut c_char) -> *mut c_void =
        unsafe { library.get(b"cef_sandbox_initialize\0") }
            .map(|symbol| *symbol)
            .map_err(|e| unavailable(format!("cef_sandbox_initialize not found: {e}")))?;

    let args: Vec<CString> = std::env::args_os()
        .filter_map(|arg| CString::new(arg.as_bytes()).ok())
        .collect();
    let mut argv: Vec<*mut c_char> = args.iter().map(|arg| arg.as_ptr().cast_mut()).collect();

    let context = unsafe { initialize(argv.len() as c_int, argv.as_mut_ptr()) };

    if context.is_null() {
        return Err(unavailable("cef_sandbox_initialize failed".into()));
    }

    let _ = HELPER_SANDBOX.set(HelperSandbox {
        _library: library,
        _context: context,
    });

    Ok(())
}

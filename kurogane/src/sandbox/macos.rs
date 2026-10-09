//! macOS Chromium sandbox.
//!
//! Chromium sandboxes macOS helpers with seatbelt profiles. Each helper enters
//! its sandbox through `libcef_sandbox.dylib` before it loads the CEF framework.

use std::error::Error;
use std::path::Path;
use std::sync::OnceLock;

use kurogane_layout::bundled_helper_path;
use tetsu::args::Args;
use tetsu::sandbox::Sandbox;

use crate::chromium_flags::ChromiumFlags;
use crate::error::RuntimeError;

/// The outcome of the helper's one attempt to enter the sandbox, held for
/// the helper's lifetime.
///
/// A failure is kept too: a partly initialized seatbelt cannot be rolled back,
/// so the attempt is never repeated.
static HELPER_SANDBOX: OnceLock<Result<Sandbox, String>> = OnceLock::new();

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

/// Enters the seatbelt sandbox through the helper's `libcef_sandbox.dylib`.
fn enter_sandbox() -> Result<Sandbox, String> {
    let describe = |error: &dyn Error| match error.source() {
        Some(source) => format!("{error}: {source}"),
        None => error.to_string(),
    };

    let mut sandbox = Sandbox::new().map_err(|e| describe(&e))?;
    sandbox
        .initialize(Args::new().as_main_args())
        .map_err(|e| describe(&e))?;

    Ok(sandbox)
}

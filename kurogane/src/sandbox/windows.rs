//! Windows Chromium sandbox.
//!
//! CEF uses `bootstrap.exe` and `bootstrapc.exe` to start a sandboxed
//! application. The bootstrap loads the application DLL, calls its entry
//! point and provides the sandbox state used by CEF.
//!
//! [`sandbox_entry!`](crate::sandbox_entry) defines the entry points.
//! [`enter`] records the bootstrap state and starts the application.
//!
//! Helper processes follow the same path through the bootstrap.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::path::Path;
use std::sync::OnceLock;

use crate::chromium_flags::ChromiumFlags;
use crate::error::RuntimeError;

/// Length of `cef_version_info_t::sandbox_compat_hash`, including its
/// terminator.
const HASH_LEN: usize = 17;

/// CEF's `cef_version_info_t`, transcribed from `cef_version_info.h`.
///
/// Declared here rather than taken from the generated bindings because it
/// crosses the bootstrap's boundary, not `libcef.dll`'s API, so nothing
/// generates it. Only the offsets and the leading `size` are read; the fields
/// are named so the layout can be checked against the header by eye.
#[repr(C)]
struct VersionInfo {
    size: usize,
    cef_version_major: c_int,
    cef_version_minor: c_int,
    cef_version_patch: c_int,
    cef_commit_number: c_int,
    chrome_version_major: c_int,
    chrome_version_minor: c_int,
    chrome_version_build: c_int,
    chrome_version_patch: c_int,
    /// Added in CEF API 14600, so present only when `size` covers it.
    sandbox_compat_hash: [c_char; HASH_LEN],
}

/// Byte offset of `sandbox_compat_hash` within [`VersionInfo`].
const HASH_OFFSET: usize = std::mem::offset_of!(VersionInfo, sandbox_compat_hash);

/// CEF's `CEF_VERSION_INFO_SIZE_WITH_SANDBOX_HASH`: the smallest reported
/// `size` that still contains `sandbox_compat_hash`.
///
/// Smaller than the padded `size_of` a current bootstrap reports and larger
/// than what a bootstrap older than CEF API 14600 reports.
const SIZE_WITH_SANDBOX_HASH: usize = HASH_OFFSET + HASH_LEN;

/// `cef_api_hash` entry selecting `CEF_SANDBOX_COMPAT_HASH`.
const API_HASH_SANDBOX_COMPAT: c_int = 3;

/// State supplied by the CEF bootstrap.
struct Bootstrap {
    /// Opaque sandbox state supplied by the bootstrap
    sandbox_info: *mut c_void,

    /// Sandbox ABI tag supplied by the bootstrap, when available
    sandbox_hash: Option<String>,
}

// SAFETY: the only field that is not `Send` is a pointer this crate never
// dereferences. It is handed back to CEF, which owns what it addresses, from
// whichever thread starts the runtime.
unsafe impl Send for Bootstrap {}

// SAFETY: `BOOTSTRAP` is written once, from the entry point and every later
// access only copies the pointer's value, which is not a use of what it
// addresses.
unsafe impl Sync for Bootstrap {}

static BOOTSTRAP: OnceLock<Bootstrap> = OnceLock::new();

pub(super) fn apply_disabled(flags: &mut ChromiumFlags) {
    flags.set("no-sandbox");
    flags.set("disable-gpu-sandbox");
}

/// Runs an application entry point under CEF's bootstrap.
///
/// `sandbox_info` and `version_info` are the arguments the bootstrap passed to
/// `RunWinMain` or `RunConsoleMain` and both stay valid for the whole call.
/// [`sandbox_entry!`](crate::sandbox_entry) reaches this through
/// `kurogane::__private`.
///
/// Returns the process exit code, 1 if `entry` panics or the process was
/// already entered. Kurogane's helper processes exit from inside `entry`, so
/// in practice only the browser process returns through here.
///
/// # Safety
///
/// The pointers must be the bootstrap's own arguments, unmodified.
pub unsafe fn enter(sandbox_info: *mut c_void, version_info: *mut c_void, entry: fn()) -> c_int {
    // SAFETY: `version_info` is this function's own argument, which the
    // caller's contract states is the bootstrap's `cef_version_info_t` (or
    // null). The bootstrap owns it for the whole of this call.
    let sandbox_hash = unsafe { read_sandbox_hash(version_info) };

    // The bootstrap enters once per process. A second entry would start a
    // second runtime on the first one's sandbox state, so it is refused
    let bootstrap = Bootstrap {
        sandbox_info,
        sandbox_hash,
    };
    if BOOTSTRAP.set(bootstrap).is_err() {
        return 1;
    }

    // The bootstrap keeps the library loaded and records the exit code, so a
    // panic is reported rather than aborted across the C boundary
    match std::panic::catch_unwind(entry) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

/// Reads `sandbox_compat_hash` out of the bootstrap's `cef_version_info_t`.
///
/// Only the leading `size` field is guaranteed to be there: a bootstrap built
/// before CEF API 14600 reports a shorter structure and reading the hash from
/// it would run past the end.
///
/// # Safety
///
/// `version_info` must be null or a `cef_version_info_t` whose `size` field
/// describes it truthfully.
unsafe fn read_sandbox_hash(version_info: *mut c_void) -> Option<String> {
    if version_info.is_null() {
        return None;
    }

    // SAFETY: `version_info` is non-null, checked above and by this
    // function's contract points at a `cef_version_info_t`. Every version of
    // that structure begins with its `size`, so those bytes are initialized
    // whatever the bootstrap's CEF version. The read is unaligned because
    // nothing here establishes the pointer's alignment.
    let size = unsafe { version_info.cast::<usize>().read_unaligned() };

    if size < SIZE_WITH_SANDBOX_HASH {
        return None;
    }

    // SAFETY: `size` is the structure's own reported length and covers
    // `HASH_OFFSET + HASH_LEN`, checked above, so the offset and the read
    // both stay inside the allocation the caller pointed at. `offset_of!`
    // places the field where the C header does. Any bit pattern is a valid
    // `[u8; HASH_LEN]`, so the bytes need not be a well-formed string here.
    let field = unsafe {
        version_info
            .cast::<u8>()
            .add(HASH_OFFSET)
            .cast::<[u8; HASH_LEN]>()
            .read_unaligned()
    };

    // CEF reserves the last byte for the terminator; a field without one is
    // not a tag
    let field = CStr::from_bytes_until_nul(&field).ok()?;

    tag(field).map(str::to_owned)
}

/// Returns the sandbox state to give CEF, or null when the application was not
/// launched by the bootstrap.
pub(super) fn sandbox_info() -> *mut u8 {
    BOOTSTRAP.get().map_or(std::ptr::null_mut(), |bootstrap| {
        bootstrap.sandbox_info.cast()
    })
}

/// Confirms the application can enter Chromium's sandbox.
///
/// Requires the bootstrap and requires it to agree with `libcef.dll` about
/// the sandbox ABI: the two are separate binaries and a mismatched pair would
/// leave the broker and its targets talking past each other.
pub(super) fn preflight(_cef_root: &Path) -> Result<(), RuntimeError> {
    let Some(bootstrap) = BOOTSTRAP.get() else {
        return Err(RuntimeError::SandboxUnsupported {
            reason: concat!(
                "the Chromium sandbox on Windows is brokered by CEF's bootstrap \
                 executable, which loads the application as a DLL.\n\n",
                "  Declare the entry points with kurogane::sandbox_entry!, give the \
                 crate a cdylib target\n",
                "  and set `sandbox = true` under [app] in kurogane.toml. \
                 `kurogane run` and `kurogane bundle`\n",
                "  then start it through the bootstrap."
            )
            .into(),
        });
    };

    if bootstrap.sandbox_info.is_null() {
        return Err(RuntimeError::SandboxUnavailable {
            reason: "CEF's bootstrap started the application without sandbox state, \
                     so there is no broker to sandbox its helpers."
                .into(),
        });
    }

    match (bootstrap.sandbox_hash.as_deref(), runtime_sandbox_hash()) {
        (Some(bootstrap_hash), Some(runtime_hash)) if bootstrap_hash == runtime_hash => Ok(()),
        // Before CEF API 14600 neither side published a tag; nothing to compare
        (None, None) => Ok(()),
        // A tag on one side only is a bootstrap from another release, or a
        // field that is present but unreadable
        (bootstrap_hash, runtime_hash) => Err(RuntimeError::SandboxUnavailable {
            reason: format!(
                concat!(
                    "The bootstrap executable and libcef.dll come from different CEF \
                     releases.\n\n",
                    "  bootstrap sandbox ABI: {}\n",
                    "  libcef.dll sandbox ABI: {}\n\n",
                    "Both must come from one CEF installation. Run `kurogane install` \
                     and rebuild."
                ),
                bootstrap_hash.unwrap_or("none"),
                runtime_hash.unwrap_or("none")
            ),
        }),
    }
}

/// Returns the sandbox ABI tag `libcef.dll` was built with.
///
/// The API version is fixed by the first `cef_api_hash` call, which loading
/// libcef makes before this one, so the argument here only has to match it.
fn runtime_sandbox_hash() -> Option<&'static str> {
    let hash = tanso::api_hash(tanso::sys::CEF_API_VERSION_LAST, API_HASH_SANDBOX_COMPAT);

    if hash.is_null() {
        return None;
    }

    // SAFETY: CEF returns a NUL-terminated string it owns and keeps for the
    // lifetime of the library, which outlives the runtime
    tag(unsafe { CStr::from_ptr(hash) })
}

/// Interprets a sandbox ABI tag, or `None` when it is blank or not UTF-8.
///
/// CEF leaves the tag empty rather than absent on platforms without one.
fn tag(text: &CStr) -> Option<&str> {
    text.to_str().ok().filter(|text| !text.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `cef_version_info_t` the bootstrap could have passed, allocated to
    /// exactly the length it reports.
    ///
    /// The exact length is the point: an older bootstrap's structure really
    /// does end early, so reading the hash out of one is a real out-of-bounds
    /// access and Miri sees it.
    fn version_info(size: usize, hash: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0u8; size];

        bytes[..size_of::<usize>()].copy_from_slice(&size.to_ne_bytes());

        if size >= SIZE_WITH_SANDBOX_HASH {
            bytes[HASH_OFFSET..HASH_OFFSET + hash.len()].copy_from_slice(hash);
        }

        bytes
    }

    #[test]
    fn the_hash_offset_follows_cef_s_layout() {
        // A `size_t`, then the eight version fields, then the hash: 40 and 57
        // on a 64-bit build. Computed by hand here so that adding or removing
        // a field in `VersionInfo` without meaning to has to fail something.
        assert_eq!(HASH_OFFSET, size_of::<usize>() + size_of::<[c_int; 8]>());
        assert_eq!(SIZE_WITH_SANDBOX_HASH, HASH_OFFSET + HASH_LEN);
    }

    #[test]
    fn a_reported_hash_is_read() {
        let mut info = version_info(SIZE_WITH_SANDBOX_HASH, b"6c5b35ad81055c14");

        // SAFETY: `info` is a live buffer whose leading `size` describes it
        let hash = unsafe { read_sandbox_hash(info.as_mut_ptr().cast()) };

        assert_eq!(hash.as_deref(), Some("6c5b35ad81055c14"));
    }

    #[test]
    fn a_structure_too_short_for_the_hash_reports_none() {
        // A bootstrap older than CEF API 14600 stops before the field
        let mut info = version_info(HASH_OFFSET, b"6c5b35ad81055c14");

        // SAFETY: `info` is a live buffer whose leading `size` describes it
        assert!(unsafe { read_sandbox_hash(info.as_mut_ptr().cast()) }.is_none());
    }

    #[test]
    fn no_version_info_reports_none() {
        // SAFETY: a null pointer is one of the two shapes the contract allows
        assert!(unsafe { read_sandbox_hash(std::ptr::null_mut()) }.is_none());
    }

    #[test]
    fn a_blank_hash_is_not_a_tag() {
        let mut info = version_info(SIZE_WITH_SANDBOX_HASH, b"");

        // SAFETY: `info` is a live buffer whose leading `size` describes it
        assert!(unsafe { read_sandbox_hash(info.as_mut_ptr().cast()) }.is_none());
    }

    #[test]
    fn an_unterminated_hash_is_refused() {
        // CEF reserves the last byte for the terminator; without one the field
        // is not a tag and comparing it would compare uninitialized bytes
        let mut info = version_info(SIZE_WITH_SANDBOX_HASH, &[b'a'; HASH_LEN]);

        // SAFETY: `info` is a live buffer whose leading `size` describes it
        assert!(unsafe { read_sandbox_hash(info.as_mut_ptr().cast()) }.is_none());
    }

    #[test]
    fn no_bootstrap_means_no_sandbox_info() {
        // The test process is not launched by the bootstrap
        assert!(sandbox_info().is_null());
    }
}

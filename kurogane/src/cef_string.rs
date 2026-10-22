//! Strings Kurogane hands to CEF in a struct a callback fills in.

use tanso::{CefString, sys};

/// Returns `value` as a string in a buffer CEF allocated, for a string field
/// of a struct a callback fills in for CEF (`LinuxWindowProperties`).
///
/// tanso writes such a struct back to CEF field by field. Its conversion
/// keeps only a string it borrowed. One made with `CefString::from(&str)`
/// owns its buffer and reaches CEF empty. This one is copied into a buffer
/// of CEF's (`cef_string_utf16_set`, which sets CEF's own destructor) that
/// the `CefString` only borrows. The write-back hands it over as it is and
/// CEF frees it with the struct. Never clone the result; a clone of a
/// borrowed `CefString` aliases its buffer. `CefString::from` does the same
/// once tanso keeps owned strings.
pub(crate) fn owned_by_cef(value: &str) -> CefString {
    let utf16: Vec<u16> = value.encode_utf16().collect();
    // SAFETY: all zero is CEF's empty string, with no buffer to free
    let mut raw: sys::_cef_string_utf16_t = unsafe { std::mem::zeroed() };
    // SAFETY: `utf16` outlives the call, which copies it (copy = 1) into a
    // buffer CEF allocates and frees through the destructor it sets in `raw`
    unsafe { sys::cef_string_utf16_set(utf16.as_ptr(), utf16.len(), &mut raw, 1) };
    CefString::from(raw)
}

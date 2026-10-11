//! Strings Kurogane hands to CEF in a struct a callback fills in.

use tanso::{CefString, sys};

/// `value` as a string in a buffer CEF allocated, for a string field of a
/// struct a callback fills in for CEF (`LinuxWindowProperties`).
///
/// cef-rs writes such a struct back to CEF field by field, and its
/// conversion keeps only a string it borrowed: one made with
/// `CefString::from(&str)` owns its buffer and reaches CEF empty (cef-rs
/// issue #409). This one is copied into a buffer of CEF's
/// (`cef_string_utf16_set`, which sets CEF's own destructor) that the
/// `CefString` only borrows, so the write-back hands it over as it is and
/// CEF frees it with the struct. Never clone the result: a clone of a
/// borrowed `CefString` aliases its buffer. Once cef-rs keeps owned
/// strings, `CefString::from` does the same.
pub(crate) fn owned_by_cef(value: &str) -> CefString {
    let utf16: Vec<u16> = value.encode_utf16().collect();
    // SAFETY: all zero is CEF's empty string, with no buffer to free
    let mut raw: sys::_cef_string_utf16_t = unsafe { std::mem::zeroed() };
    // SAFETY: `utf16` outlives the call, which copies it (copy = 1) into a
    // buffer CEF allocates and frees through the destructor it sets in `raw`
    unsafe { sys::cef_string_utf16_set(utf16.as_ptr(), utf16.len(), &mut raw, 1) };
    CefString::from(raw)
}

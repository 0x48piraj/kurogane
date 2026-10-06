use tetsu::*;

/// Formats a promise rejection as `"{code}: {message}"`, the form the
/// bridge's `toError` parses into an `Error` with a numeric `.code`.
pub fn rejection(code: i32, message: &str) -> CefString {
    CefString::from(format!("{code}: {message}").as_str())
}

/// Create a V8 ArrayBuffer by copying bytes into a new backing store.
///
/// The returned ArrayBuffer is independent of 'payload'. Empty payloads use
/// the copy-based API because the backing-store API requires a release callback.
pub fn create_array_buffer_from_bytes(payload: &[u8]) -> Option<V8Value> {
    if payload.is_empty() {
        // Does not require a release callback
        return v8_value_create_array_buffer_with_copy(std::ptr::null_mut(), 0);
    }

    let mut store = v8_backing_store_create(payload.len())?;

    if store.is_valid() == 0 {
        return None;
    }

    // SAFETY: `store.data()` is valid and writable for `payload.len()`
    // bytes throughout `store`'s lifetime. The fresh CEF allocation is
    // distinct from `payload`, satisfying `copy_nonoverlapping`.
    unsafe {
        std::ptr::copy_nonoverlapping(payload.as_ptr(), store.data() as *mut u8, payload.len());
    }

    v8_value_create_array_buffer_from_backing_store(Some(&mut store))
}

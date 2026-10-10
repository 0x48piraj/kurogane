use tetsu::*;

/// Formats a promise rejection as `"{code}: {message}"`, the form the
/// bridge's `toError` parses into an `Error` with a numeric `.code`.
pub fn rejection(code: i32, message: &str) -> CefString {
    CefString::from(format!("{code}: {message}").as_str())
}

/// Creates a V8 ArrayBuffer holding a copy of `payload`.
pub fn create_array_buffer_from_bytes(payload: &[u8]) -> Option<V8Value> {
    // CEF only reads the bytes it copies
    v8_value_create_array_buffer_with_copy(payload.as_ptr().cast_mut(), payload.len())
}

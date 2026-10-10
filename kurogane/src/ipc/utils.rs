use tetsu::*;

/// Formats a promise rejection as `"{code}: {message}"`, the form the
/// bridge's `toError` parses into an `Error` with a numeric `.code`.
pub fn rejection(code: i32, message: &str) -> CefString {
    CefString::from(format!("{code}: {message}").as_str())
}

/// Creates a V8 ArrayBuffer of `data`, wrapping `filled` which holds
/// `data`'s bytes when given.
pub fn array_buffer(data: &[u8], filled: Option<V8BackingStore>) -> Option<V8Value> {
    if let Some(mut store) = filled {
        // The inbox fills a store from the bytes the handler decodes. A store
        // of another length is a bug; the copy below stays correct
        debug_assert_eq!(
            store.byte_length(),
            data.len(),
            "a filled store of another length"
        );
        if store.byte_length() == data.len() {
            return v8_value_create_array_buffer_from_backing_store(Some(&mut store));
        }
    }
    v8_value_create_array_buffer_from_bytes(data)
}

use cef::*;

/// Create a V8 ArrayBuffer by copying bytes into a new backing store.
///
/// The returned ArrayBuffer is independent of 'payload'. Empty payloads use
/// the copy-based API because the backing-store API requires a release callback.
pub fn create_array_buffer_from_bytes(payload: &[u8]) -> Option<V8Value> {
    if payload.is_empty() {
        // Does not require a release callback
        return v8_value_create_array_buffer_with_copy(
            std::ptr::null_mut(), 0
        );
    }

    let mut store = v8_backing_store_create(payload.len())?;

    if store.is_valid() == 0 {
        return None;
    }

    unsafe {
        std::ptr::copy_nonoverlapping(
            payload.as_ptr(),
            store.data() as *mut u8,
            payload.len(),
        );
    }

    v8_value_create_array_buffer_from_backing_store(Some(&mut store))
}

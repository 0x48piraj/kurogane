//! Binary transfer subsystem with automatic SHM threshold
//!
//! Handles binary request/response with automatic switching between
//! inline and shared memory based on payload size.

use cef::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::panic::{catch_unwind, AssertUnwindSafe};
use crate::ipc::protocol::IpcMsgKind;
use crate::ipc::transport::cef_shm;
use crate::ipc::binary_buffer::SharedBinary;
use crate::ipc::renderer_state::registry;
use crate::ipc::browser_state::{IpcContext, BinaryResponder, PendingEntry};
use crate::ipc::IpcDispatcher;
use crate::debug;

/// Inline/SHM threshold: payloads >= this size use CEF shared memory.
pub const SHM_THRESHOLD: usize = 16 * 1024; // 16KB

// BROWSER SIDE

pub fn handle_invoke(
    frame: &mut Frame,
    id: i32,
    command: String,
    data: &[u8],
    dispatcher: &Arc<IpcDispatcher>,
    ctx: IpcContext,
) {
    // Check for async binary handler first
    if dispatcher.is_async_binary(&command) {
        let aborted = Arc::new(AtomicBool::new(false));
        dispatcher.insert_pending(id, PendingEntry {
            browser_id: ctx.browser_id,
            aborted: aborted.clone(),
        });
        let responder = BinaryResponder::new(Box::new({
            let aborted = aborted.clone();
            let frame = frame.clone();
            let dispatcher = Arc::clone(dispatcher);
            move |result, error_code| {
                dispatcher.remove_pending(id);
                if !aborted.load(Ordering::SeqCst) {
                    send_response(&frame, id, result, error_code);
                } else {
                    debug!("[IPC Browser] dropping binary response for canceled id={}", id);
                }
            }
        }));
        dispatcher.dispatch_async_binary(&command, data, responder);
        return;
    }

    let result = catch_unwind(AssertUnwindSafe(|| {
        dispatcher.dispatch_binary_with_context(&command, data, ctx)
    }));

    let (response, error_code) = match result {
        Ok(Ok(data)) => (Ok(data), 0),
        Ok(Err(msg)) => (Err(msg), 0),
        Err(_) => (Err("Binary handler panicked".to_string()), -1),
    };

    send_response(frame, id, response, error_code);
}

pub fn send_error(frame: &Frame, id: i32, err: String, error_code: i32) {
    if frame.is_valid() == 0 {
        return;
    }

    let mut msg = process_message_create(Some(&CefString::from("ipc"))).unwrap();
    let mut args = msg.argument_list().unwrap();

    crate::ipc::protocol::set_kind(&mut args, IpcMsgKind::Reject);
    args.set_int(1, id);
    args.set_string(2, Some(&CefString::from(err.as_str())));
    args.set_int(3, error_code);

    frame.send_process_message(ProcessId::RENDERER, Some(&mut msg));
}

pub fn send_response(
    frame: &Frame,
    id: i32,
    result: Result<Vec<u8>, String>,
    error_code: i32,
) {
    // Guard against destroyed frames
    if frame.is_valid() == 0 {
        debug!(
            "[IPC Browser] frame destroyed before binary response id={}",
            id
        );
        return;
    }

    match result {
        Ok(data) => {
            if data.len() < SHM_THRESHOLD {
                debug!("[IPC Browser] inline binary response: {} bytes", data.len());

                let mut msg = process_message_create(Some(&CefString::from("ipc"))).unwrap();
                let mut args = msg.argument_list().unwrap();
                crate::ipc::protocol::set_kind(&mut args, IpcMsgKind::BinaryResponse);
                args.set_int(1, id);

                let mut binary = binary_value_create(Some(data.as_slice())).unwrap();
                args.set_binary(2, Some(&mut binary));

                frame.send_process_message(ProcessId::RENDERER, Some(&mut msg));
            } else {
                debug!("[IPC Browser] SHM binary response: {} bytes", data.len());

                let mut msg = match cef_shm::create(
                    "ipc",
                    IpcMsgKind::BinaryResponse as i32,
                    id,
                    &data,
                ) {
                    Some(m) => m,
                    None => {
                        debug!("[IPC Browser] SHM creation failed for id={}, falling back to inline ({} bytes)", id, data.len());
                        let mut msg = process_message_create(Some(&CefString::from("ipc"))).unwrap();
                        let mut args = msg.argument_list().unwrap();
                        crate::ipc::protocol::set_kind(&mut args, IpcMsgKind::BinaryResponse);
                        args.set_int(1, id);
                        let mut binary = binary_value_create(Some(data.as_slice())).unwrap();
                        args.set_binary(2, Some(&mut binary));
                        frame.send_process_message(ProcessId::RENDERER, Some(&mut msg));
                        return;
                    }
                };

                frame.send_process_message(ProcessId::RENDERER, Some(&mut msg));
            }
        }

        Err(err) => {
            let mut msg = process_message_create(Some(&CefString::from("ipc"))).unwrap();
            let mut args = msg.argument_list().unwrap();
            crate::ipc::protocol::set_kind(&mut args, IpcMsgKind::Reject);
            args.set_int(1, id);
            args.set_string(2, Some(&CefString::from(err.as_str())));
            args.set_int(3, error_code);
            frame.send_process_message(ProcessId::RENDERER, Some(&mut msg));
        }
    }
}

// RENDERER SIDE

/// Handle inline binary response (small payload, sent via ListValue BinaryValue).
pub fn handle_response(_frame: &mut Frame, id: i32, args: &ListValue) {
    if let Some(binary) = args.binary(2) {
        let size = binary.size();
        let mut buf = vec![0u8; size];
        let written = binary.data(Some(&mut buf), 0);
        buf.truncate(written);

        debug!("[IPC Renderer] inline binary response: {} bytes", written);
        resolve_binary(id, &buf);
    }
}

/// Handle SHM-backed binary response (large payload, zero-copy to V8).
pub fn handle_response_shm(_frame: &mut Frame, region: &SharedMemoryRegion, id: i32) {
    let buffer: SharedBinary = Arc::new(
        crate::ipc::binary_buffer::ShmBinary::new(region.clone(), cef_shm::HEADER_SIZE)
    );

    debug!(
        "[IPC Renderer] SHM binary response: {} bytes",
        buffer.data().len()
    );

    resolve_binary_shm(id, buffer);
}

fn create_array_buffer_from_bytes(payload: &[u8]) -> Option<V8Value> {
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

fn resolve_binary(id: i32, payload: &[u8]) {
    let entry = registry().lock().unwrap().take(id);

    if let Some((context, promise)) = entry {
        if context.enter() == 0 {
            eprintln!("[IPC] Failed to enter V8 context for binary promise id={}", id);
            return;
        }

        match create_array_buffer_from_bytes(payload) {
            Some(mut buf) => {
                promise.resolve_promise(Some(&mut buf));
            }

            None => {
                let reject_msg = CefString::from("ERR_-2: Failed to create ArrayBuffer backing store");
                promise.reject_promise(Some(&reject_msg));
            }
        }

        context.exit(); // safe; lock not held
    }
}

fn resolve_binary_shm(id: i32, buffer: SharedBinary) {
    let entry = registry().lock().unwrap().take(id);

    if let Some((context, promise)) = entry {
        if context.enter() == 0 {
            eprintln!("[IPC] Failed to enter V8 context for binary promise id={}", id);
            return;
        }

        match create_array_buffer_from_bytes(buffer.data()) {
            Some(mut arr) => {
                promise.resolve_promise(Some(&mut arr));
            }

            None => {
                let reject_msg = CefString::from("ERR_-2: Failed to create SHM-backed ArrayBuffer");
                promise.reject_promise(Some(&reject_msg));
            }
        }

        context.exit();
    }
}

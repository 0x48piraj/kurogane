//! Binary transfer subsystem with automatic SHM threshold
//!
//! Handles binary request/response with automatic switching between
//! inline and shared memory based on payload size.

use cef::*;
use crate::ipc::protocol::{IpcMsgKind, set_kind};
use crate::ipc::transport::shm::{SharedBuffer, SHM_THRESHOLD, SHM_HEADER_SIZE};
use crate::ipc::renderer_state::{outgoing_shm, registry};
use crate::ipc::browser_state::{response_shm_store, get_dispatcher};
use crate::debug;

// BROWSER SIDE

pub fn handle_invoke(
    frame: &mut Frame,
    id: u32,
    command: String,
    data: &[u8],
) {
    let dispatcher = get_dispatcher();

    let result = std::panic::catch_unwind(|| {
        dispatcher.lock().unwrap().dispatch_binary(&command, data)
    })
    .unwrap_or_else(|_| Err("Binary handler panicked".to_string()));

    send_response(frame, id, result);
}

pub fn handle_shm_free(id: u32) {
    response_shm_store().lock().unwrap().remove(&id);
}

pub fn send_error(frame: &Frame, id: u32, err: String) {
    if frame.is_valid() == 0 {
        return;
    }

    let mut msg = process_message_create(Some(&CefString::from("ipc"))).unwrap();
    let mut args = msg.argument_list().unwrap();

    set_kind(&mut args, IpcMsgKind::Reject);
    args.set_int(1, id as i32);
    args.set_string(2, Some(&CefString::from(err.as_str())));

    frame.send_process_message(ProcessId::RENDERER, Some(&mut msg));
}

pub fn send_response(
    frame: &Frame,
    id: u32,
    result: Result<Vec<u8>, String>,
) {
    // Guard against destroyed frames
    if frame.is_valid() == 0 {
        debug!(
            "[IPC Browser] frame destroyed before binary response id={}",
            id
        );
        return;
    }

    let mut msg = process_message_create(Some(&CefString::from("ipc"))).unwrap();
    let mut args = msg.argument_list().unwrap();

    match result {
        Ok(data) => {
            set_kind(&mut args, IpcMsgKind::BinaryResponse);
            args.set_int(1, id as i32);

            if data.len() < SHM_THRESHOLD {
                debug!("[IPC Browser] inline binary response: {} bytes", data.len());

                let mut binary = binary_value_create(Some(data.as_slice())).unwrap();
                args.set_binary(2, Some(&mut binary));
            } else {
                debug!("[IPC Browser] SHM binary response: {} bytes", data.len());

                let mut shm = match SharedBuffer::create(data.len()) {
                    Ok(s) => s,
                    Err(e) => {
                        debug!("[IPC Browser] SHM create failed: {}", e);
                        return send_error(frame, id, e);
                    }
                };

                if let Err(e) = shm.write(&data) {
                    debug!("[IPC Browser] SHM write failed: {}", e);
                    return send_error(frame, id, e);
                }

                let name = shm.name();
                args.set_string(2, Some(&CefString::from(name.as_str())));
                args.set_int(3, (data.len() + SHM_HEADER_SIZE) as i32);

                // Keep SHM alive; renderer sends msg_type 5 (SHM_FREE) after reading
                response_shm_store().lock().unwrap().insert(id, shm);
            }
        }

        Err(err) => {
            set_kind(&mut args, IpcMsgKind::Reject);
            args.set_int(1, id as i32);
            args.set_string(2, Some(&CefString::from(err.as_str())));
        }
    }

    frame.send_process_message(ProcessId::RENDERER, Some(&mut msg));
}

// RENDERER SIDE

pub fn handle_response(
    frame: &mut Frame,
    id: u32,
    args: &ListValue,
) {
    // Release outgoing SHM regardless of transport used in response
    outgoing_shm().lock().unwrap().remove(&id);

    if let Some(binary) = args.binary(2) {

        let size = binary.size();
        let mut buf = vec![0u8; size];
        let written = binary.data(Some(&mut buf), 0);
        buf.truncate(written);

        debug!("[IPC Renderer] inline binary response: {} bytes", written);

        resolve_binary(id, &buf);
    } else {
        // Browser used SHM for this response
        let name: CefString = (&args.string(2)).into();
        let size = args.int(3) as usize;

        // Pass SHM slice directly; V8 performs the copy internally
        // V8 copies the data during resolve; SHM must remain valid until then
        let shm = match SharedBuffer::open(&name.to_string(), size) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[IPC] SHM open failed for id={}: {}", id, e);

                let msg = CefString::from(format!("shm transport error: {}", e).as_str());
                crate::ipc::rpc::resolve_cef_string(id, false, &msg);

                send_shm_free(frame, id);
                return;
            }
        };

        let payload = match shm.read() {
            Ok(p) => p,
            Err(e) => {
                eprintln!("[IPC] SHM read failed: {}", e);

                let msg = CefString::from(format!("shm transport error: {}", e).as_str());
                crate::ipc::rpc::resolve_cef_string(id, false, &msg);

                send_shm_free(frame, id);
                return;
            }
        };

        resolve_binary(id, payload);

        // Notify browser it can release the SHM buffer
        send_shm_free(frame, id);
    }
}

fn resolve_binary(id: u32, payload: &[u8]) {
    let entry = registry().lock().unwrap().take(id);

    if let Some((context, promise)) = entry {
        if context.enter() == 0 {
            eprintln!("[IPC] Failed to enter V8 context for binary promise id={}", id);
            return;
        }

        let mut buf = v8_value_create_array_buffer_with_copy(
            payload.as_ptr() as *mut u8,
            payload.len(),
        ).unwrap();

        promise.resolve_promise(Some(&mut buf));
        context.exit(); // safe; lock not held
    }
}

/// Notify the browser that it can release its SHM response buffer.
pub fn send_shm_free(frame: &mut Frame, id: u32) {
    let mut msg = process_message_create(Some(&CefString::from("ipc"))).unwrap();
    let mut args = msg.argument_list().unwrap();

    set_kind(&mut args, IpcMsgKind::ShmFree);
    args.set_int(1, id as i32);

    frame.send_process_message(ProcessId::BROWSER, Some(&mut msg));
    debug!("[IPC Renderer] SHM_FREE sent for id={}", id);
}

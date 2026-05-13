//! IPC message router
//!
//! Central dispatch layer for all IPC messages between browser and renderer.

use cef::*;
use std::sync::Arc;
use crate::ipc::protocol::{get_kind, IpcMsgKind};
use crate::ipc::browser_state::IpcDispatcher;
use crate::ipc::renderer_state::outgoing_shm;
use crate::ipc::{rpc, binary};
use crate::debug;

pub fn route_browser(
    frame: &mut Frame,
    args: &ListValue,
    dispatcher: &Arc<IpcDispatcher>,
) -> bool {
    let kind = match get_kind(args) {
        Some(k) => k,
        None => {
            debug!("[IPC Router] invalid ipc message type");
            return false;
        }
    };
    let id = list_get_int(args, 1);
    if id <= 0 {
        debug!("[IPC Router] invalid id {}", id);
        return false;
    }

    debug!("[IPC Browser] message type={:?} id={}", kind, id);

    match kind {
        // JSON invoke
        IpcMsgKind::Invoke => {
            let command = list_get_string(args, 2);
            let payload = list_get_string(args, 3);

            rpc::handle_invoke(frame, id, command, payload, dispatcher);
        }

        // Binary invoke
        IpcMsgKind::BinaryInvoke => {
            let command = list_get_string(args, 2);

            // CEF exposes binary via an internal buffer; copy into Vec<u8> to own the data
            if let Some(bin) = args.binary(3) {
                let mut buf = vec![0u8; bin.size()];
                let written = bin.data(Some(&mut buf), 0);
                buf.truncate(written);

                debug!("[IPC Browser] inline binary: {} bytes", written);

                binary::handle_invoke(frame, id, command, &buf, dispatcher);
                return true;
            }

            // Large payload via SHM; open before the renderer drops it
            // Handle SHM buffer reading for large payloads
            let result: Result<(), String> = (|| {
                let name = list_get_string(args, 3);
                let raw_size = list_get_int(args, 4);

                if raw_size <= 0 {
                    return Err(format!("invalid SHM size: {}", raw_size));
                }

                let size = raw_size as usize;

                if size > crate::ipc::transport::shm::MAX_SHM_SIZE {
                    return Err(format!(
                        "SHM exceeds limit: {} > {}",
                        size,
                        crate::ipc::transport::shm::MAX_SHM_SIZE
                    ));
                }

                let shm = crate::ipc::transport::shm::SharedBuffer::open(&name, size)?;

                shm.with_read(|data| {
                    debug!(
                        "[IPC Browser] binary invoke (SHM): '{}' (id={}, {} bytes)",
                        command, id, data.len()
                    );

                    binary::handle_invoke(frame, id, command, data, dispatcher);
                })?;

                Ok(())
            })();

            if let Err(e) = result {
                debug!("[IPC Browser] SHM failure: {}", e);
                binary::send_error(frame, id, e);
            }

            return true;
        }

        // SHM_FREE: renderer has finished reading a large binary response
        IpcMsgKind::ShmFree => {
            debug!("[IPC Browser] SHM_FREE for id={}", id);
            binary::handle_shm_free(id);
        }

        _ => return false,
    }

    true
}

pub fn route_renderer(
    frame: &mut Frame,
    args: &ListValue,
) -> bool {
    let kind = match get_kind(args) {
        Some(k) => k,
        None => {
            debug!("[IPC Router] invalid ipc message type");
            return false;
        }
    };
    let id = list_get_int(args, 1);
    if id <= 0 {
        debug!("[IPC Router] invalid id {}", id);
        return false;
    }

    match kind {
        IpcMsgKind::Resolve => {
            // Release outgoing SHM; browser has read it and responded
            outgoing_shm().lock().unwrap().remove(&id);
            let payload = list_cef_string(args, 2);
            rpc::resolve_cef_string(id, true, &payload);
        }

        IpcMsgKind::Reject => {
            // Release outgoing SHM; browser has read it and responded
            outgoing_shm().lock().unwrap().remove(&id);
            let payload = list_cef_string(args, 2);
            rpc::resolve_cef_string(id, false, &payload);
        }

        IpcMsgKind::BinaryResponse => {
            binary::handle_response(frame, id, args);
        }

        _ => return false,
    }

    true
}

//
// Message helpers
//

fn list_get_int(args: &ListValue, idx: usize) -> i32 {
    // binding exposes .int(index)
    args.int(idx)
}

fn list_get_string(args: &ListValue, idx: usize) -> String {
    // binding exposes .string(index) -> CefStringUserfree
    let userfree = args.string(idx);
    // Convert to CefString (borrow conversion) then to Rust String
    let cef: CefString = (&userfree).into();
    cef.to_string()
}

//
// Helpers
//

#[inline(always)]
fn list_cef_string(args: &ListValue, idx: usize) -> CefString {
    (&args.string(idx)).into()
}

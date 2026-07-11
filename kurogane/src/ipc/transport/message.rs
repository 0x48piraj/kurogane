use std::sync::Arc;

use cef::*;

use crate::ipc::envelope::*;
use crate::ipc::binary_buffer::{ShmBinary};
use crate::ipc::binary_buffer::SharedBinary;

/// Minimum message size for shared-memory transport.
///
/// Smaller messages are sent inline.
pub const SHM_THRESHOLD: usize = 16 * 1024;

/// Message data received over the CEF transport.
///
/// Inline messages are decoded from CEF ListValue fields.
/// Shared-memory messages retain their shared representation.
pub enum ReceivedMessage {
    /// Inline message data.
    Inline {
        envelope: Envelope,
        payload: Vec<u8>,
    },
    /// Shared-memory-backed message data.
    Shm(SharedBinary),
}

impl ReceivedMessage {
    /// Returns the decoded envelope and a reference to the payload bytes.
    pub fn as_envelope_payload(&self) -> (Envelope, &[u8]) {
        match self {
            Self::Inline { envelope, payload } => (*envelope, payload.as_slice()),
            Self::Shm(b) => {
                let data = b.data();
                let (envelope, payload) = parse_envelope(data)
                    .expect("SHM message should always have a valid envelope");
                (envelope, payload)
            }
        }
    }
}

/// Builds a ProcessMessage from an envelope and payload.
///
/// Uses shared-memory transport for large messages and falls back to
/// inline transport if shared memory is unavailable.
pub fn build_message(
    name: &str,
    envelope: &Envelope,
    payload: &[u8],
) -> Option<ProcessMessage> {
    build_message_parts(name, envelope, &[payload])
}

/// Builds a ProcessMessage from an envelope and payload segments.
///
/// The payload is assembled directly from the provided slices.
pub fn build_message_parts(
    name: &str,
    envelope: &Envelope,
    parts: &[&[u8]],
) -> Option<ProcessMessage> {
    let total_payload: usize = parts.iter().map(|p| p.len()).sum();
    let total_size = ENVELOPE_SIZE + total_payload;

    if total_size < SHM_THRESHOLD {
        build_inline_parts(name, envelope, parts)
    } else {
        build_shm_parts(name, envelope, parts)
            .or_else(|| build_inline_parts(name, envelope, parts))
    }
}

/// Build an inline ProcessMessage using CEF ListValue fields.
///
/// Envelope fields are stored as individual ListValue entries to avoid
/// an extra flat-buffer serialization. CEF's native IPC transports
/// the structured ListValue directly.
fn build_inline_parts(
    name: &str,
    envelope: &Envelope,
    parts: &[&[u8]],
) -> Option<ProcessMessage> {
    let msg = process_message_create(Some(&CefString::from(name)))?;
    let args = msg.argument_list()?;

    args.set_int(0, envelope.version as i32);
    args.set_int(1, envelope.subsystem as i32);
    args.set_int(2, envelope.opcode as i32);
    args.set_int(3, envelope.flags as i32);
    args.set_int(4, envelope.correlation_id as i32);
    args.set_int(5, envelope.payload_kind as i32);

    let total_payload: usize = parts.iter().map(|p| p.len()).sum();
    if total_payload > 0 {
        if parts.len() == 1 {
            let mut binary = binary_value_create(Some(parts[0]))?;
            args.set_binary(6, Some(&mut binary));
        } else {
            let mut buf = Vec::with_capacity(total_payload);
            for part in parts {
                buf.extend_from_slice(part);
            }
            let mut binary = binary_value_create(Some(&buf))?;
            args.set_binary(6, Some(&mut binary));
        }
    }

    Some(msg)
}

fn build_shm_parts(
    name: &str,
    envelope: &Envelope,
    parts: &[&[u8]],
) -> Option<ProcessMessage> {
    let total_payload: usize = parts.iter().map(|p| p.len()).sum();
    let total_size = ENVELOPE_SIZE + total_payload;
    let builder = shared_process_message_builder_create(
        Some(&CefString::from(name)),
        total_size,
    )?;
    if builder.is_valid() == 0 {
        return None;
    }

    unsafe {
        let ptr = builder.memory() as *mut u8;
        let env_bytes = encode_envelope_bytes(envelope);
        std::ptr::copy_nonoverlapping(env_bytes.as_ptr(), ptr, ENVELOPE_SIZE);
        let mut offset = ENVELOPE_SIZE;
        for part in parts {
            std::ptr::copy_nonoverlapping(part.as_ptr(), ptr.add(offset), part.len());
            offset += part.len();
        }
    }

    builder.build()
}

/// Extracts a serialized message from a ProcessMessage.
///
/// Returns either shared-memory-backed or inline storage from CEF ListValue fields.
pub fn extract_message(message: &ProcessMessage) -> Option<ReceivedMessage> {
    // SHM path: zero-copy from shared memory
    if let Some(region) = message.shared_memory_region() {
        if region.is_valid() != 0 && region.size() >= ENVELOPE_SIZE {
            return Some(ReceivedMessage::Shm(Arc::new(ShmBinary::new(region, 0))));
        }
    }

    // Inline path: read from ListValue fields
    let args = message.argument_list()?;

    let envelope = Envelope {
        version: args.int(0) as u8,
        subsystem: args.int(1) as u8,
        opcode: args.int(2) as u8,
        flags: args.int(3) as u8,
        correlation_id: args.int(4) as u32,
        payload_kind: args.int(5) as u8,
    };

    if envelope.version != ENVELOPE_VERSION {
        return None;
    }

    let payload = if let Some(binary) = args.binary(6) {
        let size = binary.size();
        let mut buf = vec![0u8; size];
        let written = binary.data(Some(&mut buf), 0);
        buf.truncate(written);
        buf
    } else {
        Vec::new()
    };

    Some(ReceivedMessage::Inline { envelope, payload })
}

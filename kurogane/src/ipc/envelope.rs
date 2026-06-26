//! IPC wire protocol.
//!
//! Defines the common envelope format, protocol identifiers, opcode
//! assignments and helpers for encoding and decoding messages.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Envelope {
    pub version: u8,
    pub subsystem: u8,
    pub opcode: u8,
    pub flags: u8,
    pub correlation_id: u32,
    pub payload_kind: u8,
}

/// Current wire protocol version.
///
/// Peers reject messages with an unsupported version.
pub const ENVELOPE_VERSION: u8 = 2;

/// Fixed-size envelope header (9 bytes).
/// version (1) | subsystem (1) | opcode (1) | flags (1) | correlation_id (4 LE u32) | payload_kind (1)
pub const ENVELOPE_SIZE: usize = 9;

/// Protocol subsystem identifiers.
pub const SUB_RPC: u8 = 0;
pub const SUB_BINARY: u8 = 1;
pub const SUB_EVENT: u8 = 2;
pub const SUB_STREAM: u8 = 3;

/// RPC protocol opcodes.
pub const RPC_INVOKE: u8 = 0;
pub const RPC_RESOLVE: u8 = 1;
pub const RPC_REJECT: u8 = 2;
pub const RPC_CANCEL: u8 = 3;

/// Binary transfer protocol opcodes.
pub const BINARY_INVOKE: u8 = 0;
pub const BINARY_RESPONSE: u8 = 1;
pub const BINARY_REJECT: u8 = 2;
pub const BINARY_CANCEL: u8 = 3;

/// Event protocol opcodes.
/// Renderers subscribe to events. Browsers emit events to subscribed frames.
pub const EVENT_SUBSCRIBE: u8 = 0;
pub const EVENT_UNSUBSCRIBE: u8 = 1;
pub const EVENT_EMIT: u8 = 2;

/// Stream protocol opcodes.
/// Stream messages are currently renderer-to-browser only.
pub const STREAM_OPEN: u8 = 0;
pub const STREAM_DATA: u8 = 1;
pub const STREAM_END: u8 = 2;
pub const STREAM_ERROR: u8 = 3;
pub const STREAM_CANCEL: u8 = 4;

/// Payload encoding identifiers.
pub const PAYLOAD_EMPTY: u8 = 0;
pub const PAYLOAD_STRING: u8 = 1;
pub const PAYLOAD_BINARY: u8 = 2;
pub const PAYLOAD_JSON: u8 = 3;

/// Envelope flag bits.
pub const FLAG_PERSISTENT: u8 = 0b0000_0001;

/// Encodes an envelope into its serialized wire representation.
#[inline]
pub fn encode_envelope_bytes(envelope: &Envelope) -> [u8; ENVELOPE_SIZE] {
    let mut buf = [0u8; ENVELOPE_SIZE];
    buf[0] = envelope.version;
    buf[1] = envelope.subsystem;
    buf[2] = envelope.opcode;
    buf[3] = envelope.flags;
    buf[4..8].copy_from_slice(&envelope.correlation_id.to_le_bytes());
    buf[8] = envelope.payload_kind;
    buf
}

/// Decodes an envelope from its wire representation.
///
/// Returns the decoded envelope together with the remaining payload.
/// Returns None if the message is truncated or uses an unsupported protocol version.
#[inline]
pub fn parse_envelope(data: &[u8]) -> Option<(Envelope, &[u8])> {
    if data.len() < ENVELOPE_SIZE {
        return None;
    }
    let envelope = Envelope {
        version: data[0],
        subsystem: data[1],
        opcode: data[2],
        flags: data[3],
        correlation_id: u32::from_le_bytes([data[4], data[5], data[6], data[7]]),
        payload_kind: data[8],
    };
    if envelope.version != ENVELOPE_VERSION {
        return None;
    }
    Some((envelope, &data[ENVELOPE_SIZE..]))
}

/// Encodes a command payload.
///
/// Wire format: [command_len: u16 LE][command UTF-8][data]
pub fn encode_cmd_payload(cmd: &str, data: &[u8]) -> Vec<u8> {
    let cmd_bytes = cmd.as_bytes();
    let mut buf = Vec::with_capacity(2 + cmd_bytes.len() + data.len());
    buf.extend_from_slice(&(cmd_bytes.len() as u16).to_le_bytes());
    buf.extend_from_slice(cmd_bytes);
    buf.extend_from_slice(data);
    buf
}

/// Decodes a command payload.
///
/// Returns the command and remaining data.
/// Returns None if the payload is malformed or the command is not valid UTF-8.
pub fn decode_cmd_payload(payload: &[u8]) -> Option<(&str, &[u8])> {
    if payload.len() < 2 {
        return None;
    }
    let cmd_len = u16::from_le_bytes([payload[0], payload[1]]) as usize;
    if 2 + cmd_len > payload.len() {
        return None;
    }
    let cmd = std::str::from_utf8(&payload[2..2 + cmd_len]).ok()?;
    Some((cmd, &payload[2 + cmd_len..]))
}

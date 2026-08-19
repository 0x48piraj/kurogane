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
pub const SUB_EVENT: u8 = 1;
pub const SUB_STREAM: u8 = 2;

/// RPC protocol opcodes.
pub const RPC_INVOKE: u8 = 0;
pub const RPC_RESOLVE: u8 = 1;
pub const RPC_REJECT: u8 = 2;
pub const RPC_CANCEL: u8 = 3;

/// Event protocol opcodes.
/// Renderers subscribe to events. Browsers emit events to subscribed frames.
pub const EVENT_SUBSCRIBE: u8 = 0;
pub const EVENT_UNSUBSCRIBE: u8 = 1;
pub const EVENT_EMIT: u8 = 2;

/// Stream protocol opcodes.
pub const STREAM_OPEN: u8 = 0;
pub const STREAM_DATA: u8 = 1;
pub const STREAM_END: u8 = 2;
pub const STREAM_ERROR: u8 = 3;
pub const STREAM_CANCEL: u8 = 4;

pub const STREAM_BROWSER_DATA: u8 = 5;
pub const STREAM_BROWSER_END: u8 = 6;
pub const STREAM_BROWSER_ERROR: u8 = 7;

/// Payload encoding identifiers.
pub const PAYLOAD_EMPTY: u8 = 0;
pub const PAYLOAD_STRING: u8 = 1;
pub const PAYLOAD_BINARY: u8 = 2;
pub const PAYLOAD_JSON: u8 = 3;

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

#[cfg(test)]
mod tests {
    use super::*;

    // Envelope encode / parse roundtrip: encoding then parsing an envelope reproduces the original
    #[test]
    fn envelope_roundtrip_minimal() {
        let env = Envelope {
            version: ENVELOPE_VERSION,
            subsystem: SUB_RPC,
            opcode: RPC_INVOKE,
            flags: 0,
            correlation_id: 0,
            payload_kind: PAYLOAD_EMPTY,
        };
        let buf = encode_envelope_bytes(&env);
        assert_eq!(buf.len(), ENVELOPE_SIZE);

        let (decoded, rest) = parse_envelope(&buf).expect("parse must succeed");
        assert_eq!(decoded, env);
        assert!(rest.is_empty(), "no trailing data expected");
    }

    // Trailing bytes after the 9-byte header are returned as the remaining slice.
    #[test]
    fn envelope_roundtrip_with_payload() {
        let env = Envelope {
            version: ENVELOPE_VERSION,
            subsystem: SUB_EVENT,
            opcode: EVENT_EMIT,
            flags: 0xFF,
            correlation_id: 42,
            payload_kind: PAYLOAD_JSON,
        };
        let mut buf = encode_envelope_bytes(&env).to_vec();
        buf.extend_from_slice(b"extra payload data");

        let (decoded, rest) = parse_envelope(&buf).expect("parse must succeed");
        assert_eq!(decoded, env);
        assert_eq!(rest, b"extra payload data");
    }

    #[test]
    fn envelope_roundtrip_all_subsystems() {
        for sub in [SUB_RPC, SUB_EVENT, SUB_STREAM] {
            let env = Envelope {
                version: ENVELOPE_VERSION,
                subsystem: sub,
                opcode: 0,
                flags: 0,
                correlation_id: 0,
                payload_kind: PAYLOAD_EMPTY,
            };
            let buf = encode_envelope_bytes(&env);
            let (decoded, _) = parse_envelope(&buf).unwrap();
            assert_eq!(decoded.subsystem, sub);
        }
    }

    #[test]
    fn envelope_roundtrip_all_payload_kinds() {
        for kind in [PAYLOAD_EMPTY, PAYLOAD_STRING, PAYLOAD_BINARY, PAYLOAD_JSON] {
            let env = Envelope {
                version: ENVELOPE_VERSION,
                subsystem: SUB_RPC,
                opcode: 0,
                flags: 0,
                correlation_id: 0,
                payload_kind: kind,
            };
            let buf = encode_envelope_bytes(&env);
            let (decoded, _) = parse_envelope(&buf).unwrap();
            assert_eq!(decoded.payload_kind, kind);
        }
    }

    #[test]
    fn envelope_roundtrip_max_correlation_id() {
        let env = Envelope {
            version: ENVELOPE_VERSION,
            subsystem: SUB_RPC,
            opcode: RPC_INVOKE,
            flags: 0,
            correlation_id: u32::MAX,
            payload_kind: PAYLOAD_EMPTY,
        };
        let buf = encode_envelope_bytes(&env);
        let (decoded, _) = parse_envelope(&buf).unwrap();
        assert_eq!(decoded.correlation_id, u32::MAX);
    }

    #[test]
    fn envelope_roundtrip_zero_correlation_id() {
        let env = Envelope {
            version: ENVELOPE_VERSION,
            subsystem: SUB_STREAM,
            opcode: STREAM_OPEN,
            flags: 0,
            correlation_id: 0,
            payload_kind: PAYLOAD_EMPTY,
        };
        let buf = encode_envelope_bytes(&env);
        let (decoded, _) = parse_envelope(&buf).unwrap();
        assert_eq!(decoded.correlation_id, 0);
    }

    #[test]
    fn envelope_wire_format_is_little_endian() {
        let env = Envelope {
            version: ENVELOPE_VERSION,
            subsystem: 0,
            opcode: 0,
            flags: 0,
            correlation_id: 0x01020304,
            payload_kind: 0,
        };
        let buf = encode_envelope_bytes(&env);
        // bytes 4..8 should be LE encoding of 0x01020304
        assert_eq!(buf[4], 0x04);
        assert_eq!(buf[5], 0x03);
        assert_eq!(buf[6], 0x02);
        assert_eq!(buf[7], 0x01);
    }

    // Header fields occupy their documented byte positions.
    #[test]
    fn envelope_header_layout() {
        let env = Envelope {
            version: ENVELOPE_VERSION,
            subsystem: SUB_STREAM,
            opcode: STREAM_DATA,
            flags: 0x42,
            correlation_id: 7,
            payload_kind: PAYLOAD_BINARY,
        };
        let buf = encode_envelope_bytes(&env);
        assert_eq!(buf[0], ENVELOPE_VERSION, "version at byte 0");
        assert_eq!(buf[1], SUB_STREAM, "subsystem at byte 1");
        assert_eq!(buf[2], STREAM_DATA, "opcode at byte 2");
        assert_eq!(buf[3], 0x42, "flags at byte 3");
        assert_eq!(
            &buf[4..8],
            &7u32.to_le_bytes(),
            "correlation_id at bytes 4..8"
        );
        assert_eq!(buf[8], PAYLOAD_BINARY, "payload_kind at byte 8");
    }

    // Zero-length input cannot contain a valid envelope
    #[test]
    fn parse_envelope_empty_returns_none() {
        assert!(parse_envelope(&[]).is_none());
    }

    #[test]
    fn parse_envelope_one_byte_returns_none() {
        assert!(parse_envelope(&[0u8]).is_none());
    }

    #[test]
    fn parse_envelope_four_bytes_returns_none() {
        assert!(parse_envelope(&[0u8; 4]).is_none());
    }

    #[test]
    fn parse_envelope_eight_bytes_returns_none() {
        assert!(parse_envelope(&[0u8; 8]).is_none());
    }

    #[test]
    fn parse_envelope_exactly_envelope_size_returns_none_for_wrong_version() {
        let mut buf = [0u8; ENVELOPE_SIZE];
        buf[0] = ENVELOPE_VERSION + 1; // wrong version
        assert!(parse_envelope(&buf).is_none());
    }

    #[test]
    fn parse_envelope_wrong_version_returns_none() {
        for v in [0u8, 1, 3, 127, 255] {
            if v == ENVELOPE_VERSION {
                continue;
            }
            let mut buf = [0u8; ENVELOPE_SIZE];
            buf[0] = v;
            assert!(
                parse_envelope(&buf).is_none(),
                "version {} should be rejected",
                v
            );
        }
    }

    #[test]
    fn parse_envelope_version_zero_rejected() {
        let mut buf = [0u8; ENVELOPE_SIZE];
        buf[0] = 0;
        assert!(parse_envelope(&buf).is_none());
    }

    // Encode then decode reproduces the original (cmd, data)
    #[test]
    fn cmd_payload_roundtrip_basic() {
        let cmd = "hello";
        let data = b"world";
        let buf = encode_cmd_payload(cmd, data);
        let (decoded_cmd, decoded_data) = decode_cmd_payload(&buf).expect("decode must succeed");
        assert_eq!(decoded_cmd, cmd);
        assert_eq!(decoded_data, data);
    }

    #[test]
    fn cmd_payload_roundtrip_empty_command() {
        let cmd = "";
        let data = b"some data";
        let buf = encode_cmd_payload(cmd, data);
        let (decoded_cmd, decoded_data) = decode_cmd_payload(&buf).unwrap();
        assert_eq!(decoded_cmd, "");
        assert_eq!(decoded_data, data);
    }

    #[test]
    fn cmd_payload_roundtrip_empty_data() {
        let cmd = "my_command";
        let data = b"";
        let buf = encode_cmd_payload(cmd, data);
        let (decoded_cmd, decoded_data) = decode_cmd_payload(&buf).unwrap();
        assert_eq!(decoded_cmd, cmd);
        assert!(decoded_data.is_empty());
    }

    #[test]
    fn cmd_payload_roundtrip_both_empty() {
        let buf = encode_cmd_payload("", b"");
        let (decoded_cmd, decoded_data) = decode_cmd_payload(&buf).unwrap();
        assert_eq!(decoded_cmd, "");
        assert!(decoded_data.is_empty());
    }

    // Command lengths up to u16::MAX are supported
    #[test]
    fn cmd_payload_roundtrip_long_command() {
        let cmd = "a".repeat(1000);
        let data = b"payload";
        let buf = encode_cmd_payload(&cmd, data);
        let (decoded_cmd, decoded_data) = decode_cmd_payload(&buf).unwrap();
        assert_eq!(decoded_cmd, cmd.as_str());
        assert_eq!(decoded_data, data);
    }

    #[test]
    fn cmd_payload_roundtrip_binary_data() {
        let cmd = "bin";
        let data: Vec<u8> = (0..=255).collect();
        let buf = encode_cmd_payload(cmd, &data);
        let (decoded_cmd, decoded_data) = decode_cmd_payload(&buf).unwrap();
        assert_eq!(decoded_cmd, cmd);
        assert_eq!(decoded_data, &data[..]);
    }

    // 2-byte length prefix encodes command byte length as LE u16
    #[test]
    fn cmd_payload_wire_format_header_is_u16_le() {
        let cmd = "AB"; // 2 bytes
        let buf = encode_cmd_payload(cmd, b"");
        assert_eq!(buf[0], 2, "low byte of command length");
        assert_eq!(buf[1], 0, "high byte of command length");
    }

    // Wire format: [cmd_len:u16 LE][cmd_bytes][data_bytes]
    #[test]
    fn cmd_payload_wire_format_concatenation() {
        let cmd = "X";
        let data = b"YZ";
        let buf = encode_cmd_payload(cmd, data);
        assert_eq!(&buf[0..2], &[1, 0]); // cmd_len = 1
        assert_eq!(buf[2], b'X'); // cmd byte
        assert_eq!(&buf[3..], b"YZ"); // data bytes
    }

    // Zero-length payload cannot contain a command header
    #[test]
    fn decode_cmd_payload_empty_returns_none() {
        assert!(decode_cmd_payload(&[]).is_none());
    }

    // Payload shorter than 2 bytes has no length header
    #[test]
    fn decode_cmd_payload_one_byte_returns_none() {
        assert!(decode_cmd_payload(&[0u8]).is_none());
    }

    #[test]
    fn decode_cmd_payload_length_exceeds_payload_returns_none() {
        let mut buf = [0u8; 5];
        buf[0] = 100; // cmd_len low byte
        buf[1] = 0; // cmd_len high byte
        assert!(decode_cmd_payload(&buf).is_none());
    }

    #[test]
    fn decode_cmd_payload_length_equals_payload_length_minus_header() {
        let mut buf = [0u8; 4]; // 2 header + 2 command
        buf[0] = 2; // cmd_len = 2
        buf[1] = 0;
        buf[2] = b'a';
        buf[3] = b'b';
        let (cmd, rest) = decode_cmd_payload(&buf).unwrap();
        assert_eq!(cmd, "ab");
        assert!(rest.is_empty());
    }

    // Non-UTF-8 command bytes cause decode to fail
    #[test]
    fn decode_cmd_payload_invalid_utf8_returns_none() {
        let mut buf = [0u8; 4];
        buf[0] = 2; // cmd_len = 2
        buf[1] = 0;
        buf[2] = 0xFF; // invalid UTF-8
        buf[3] = 0xFE;
        assert!(decode_cmd_payload(&buf).is_none());
    }

    // Multi-byte UTF-8 commands are accepted
    #[test]
    fn decode_cmd_payload_valid_utf8_multibyte() {
        let cmd = "日本語"; // 3-byte UTF-8 chars
        let buf = encode_cmd_payload(cmd, b"");
        let (decoded, rest) = decode_cmd_payload(&buf).unwrap();
        assert_eq!(decoded, cmd);
        assert!(rest.is_empty());
    }

    #[test]
    fn decode_cmd_payload_with_trailing_data() {
        let buf = encode_cmd_payload("cmd", b"trailing");
        let (cmd, rest) = decode_cmd_payload(&buf).unwrap();
        assert_eq!(cmd, "cmd");
        assert_eq!(rest, b"trailing");
    }

    // Subsystem ID 255 (max u8); unused but valid wire format
    #[test]
    fn envelope_roundtrip_high_subsystem_id() {
        let env = Envelope {
            version: ENVELOPE_VERSION,
            subsystem: 0xFF,
            opcode: 0xFF,
            flags: 0xFF,
            correlation_id: u32::MAX,
            payload_kind: 0xFF,
        };
        let buf = encode_envelope_bytes(&env);
        let (decoded, _) = parse_envelope(&buf).expect("parse must succeed");
        assert_eq!(decoded.subsystem, 0xFF);
        assert_eq!(decoded.opcode, 0xFF);
        assert_eq!(decoded.flags, 0xFF);
        assert_eq!(decoded.correlation_id, u32::MAX);
        assert_eq!(decoded.payload_kind, 0xFF);
    }

    // Flags field with high bit set (0x80); currently unused by dispatch but must survive the wire format
    #[test]
    fn envelope_flags_high_bit_set() {
        let env = Envelope {
            version: ENVELOPE_VERSION,
            subsystem: SUB_RPC,
            opcode: RPC_INVOKE,
            flags: 0x80,
            correlation_id: 42,
            payload_kind: PAYLOAD_JSON,
        };
        let buf = encode_envelope_bytes(&env);
        let (decoded, _) = parse_envelope(&buf).expect("parse must succeed");
        assert_eq!(decoded.flags, 0x80);
    }

    // Each individual flag bit survives roundtrip
    #[test]
    fn envelope_all_flags_bits() {
        for bit in 0..8 {
            let flags = 1u8 << bit;
            let env = Envelope {
                version: ENVELOPE_VERSION,
                subsystem: SUB_RPC,
                opcode: RPC_INVOKE,
                flags,
                correlation_id: 0,
                payload_kind: PAYLOAD_EMPTY,
            };
            let buf = encode_envelope_bytes(&env);
            let (decoded, _) = parse_envelope(&buf).unwrap();
            assert_eq!(
                decoded.flags, flags,
                "flag bit {bit} must survive roundtrip"
            );
        }
    }

    #[test]
    fn envelope_roundtrip_with_trailing_data() {
        let env = Envelope {
            version: ENVELOPE_VERSION,
            subsystem: SUB_EVENT,
            opcode: EVENT_EMIT,
            flags: 0,
            correlation_id: 99,
            payload_kind: PAYLOAD_JSON,
        };
        let mut buf = encode_envelope_bytes(&env).to_vec();
        buf.extend_from_slice(b"extra payload data");
        let (decoded, rest) = parse_envelope(&buf).expect("parse must succeed");
        assert_eq!(decoded, env);
        assert_eq!(rest, b"extra payload data");
    }

    #[test]
    fn encode_envelope_is_little_endian() {
        let env = Envelope {
            version: ENVELOPE_VERSION,
            subsystem: SUB_RPC,
            opcode: RPC_INVOKE,
            flags: 0,
            correlation_id: 0x01020304, // little-endian
            payload_kind: PAYLOAD_EMPTY,
        };
        let buf = encode_envelope_bytes(&env);
        assert_eq!(buf[4], 0x04);
        assert_eq!(buf[5], 0x03);
        assert_eq!(buf[6], 0x02);
        assert_eq!(buf[7], 0x01);
    }

    // Both empty: header is [0x00, 0x00]; no command or data bytes
    #[test]
    fn cmd_payload_empty_command_and_data() {
        let buf = encode_cmd_payload("", b"");
        assert_eq!(buf.len(), 2);
        assert_eq!(buf[0], 0x00);
        assert_eq!(buf[1], 0x00);
        let (cmd, rest) = decode_cmd_payload(&buf).unwrap();
        assert_eq!(cmd, "");
        assert!(rest.is_empty());
    }

    // Verifies round-trip encoding for a command at the u16 length limit
    #[test]
    fn cmd_payload_max_command_length() {
        let cmd = "x".repeat(65535);
        let buf = encode_cmd_payload(&cmd, b"");
        let (decoded_cmd, rest) = decode_cmd_payload(&buf).unwrap();
        assert_eq!(decoded_cmd.len(), 65535);
        assert_eq!(decoded_cmd, cmd.as_str());
        assert!(rest.is_empty());
    }

    // Large data payload with small command
    #[test]
    fn cmd_payload_large_data() {
        let data = vec![0xAB; 50_000];
        let buf = encode_cmd_payload("go", &data);
        let (cmd, rest) = decode_cmd_payload(&buf).unwrap();
        assert_eq!(cmd, "go");
        assert_eq!(rest.len(), 50_000);
        assert!(rest.iter().all(|&b| b == 0xAB));
    }

    // All 256 byte values survive through encode/decode
    #[test]
    fn cmd_payload_binary_data_preserves_all_bytes() {
        let data: Vec<u8> = (0..=255).collect();
        let buf = encode_cmd_payload("bin", &data);
        let (_, rest) = decode_cmd_payload(&buf).unwrap();
        assert_eq!(rest, &data[..]);
    }

    // Verifies decoding rejects a truncated command payload
    #[test]
    fn decode_cmd_payload_length_header_corrupt() {
        let mut buf = vec![0u8; 2 + 5]; // 7 bytes total
        buf[0..2].copy_from_slice(&100u16.to_le_bytes()); // claims 100 bytes
        buf[2..7].copy_from_slice(b"hello");
        assert!(decode_cmd_payload(&buf).is_none());
    }

    // Verifies decoding preserves payload data when the command is empty
    #[test]
    fn decode_cmd_payload_length_header_zero_with_data() {
        let buf = encode_cmd_payload("", b"trailing");
        let (cmd, rest) = decode_cmd_payload(&buf).unwrap();
        assert_eq!(cmd, "");
        assert_eq!(rest, b"trailing");
    }
}

#[cfg(test)]
mod property_tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        // Property: encoding and then parsing an envelope preserves all fields
        #[test]
        fn prop_envelope_roundtrip(
            subsystem in prop_oneof![
                Just(SUB_RPC),
                Just(SUB_EVENT),
                Just(SUB_STREAM),
            ],
            opcode in 0u8..=10,
            flags in 0u8..=0xFF,
            correlation_id in 0u32..=u32::MAX,
            payload_kind in 0u8..=3u8,
        ) {
            let env = Envelope {
                version: ENVELOPE_VERSION,
                subsystem,
                opcode,
                flags,
                correlation_id,
                payload_kind,
            };
            let buf = encode_envelope_bytes(&env);
            let (decoded, rest) = parse_envelope(&buf).expect("parse must succeed for valid envelope");
            prop_assert_eq!(&decoded, &env);
            prop_assert!(rest.is_empty());
        }

        // Property: parsing always fails for inputs shorter than ENVELOPE_SIZE.
        #[test]
        fn prop_envelope_truncated_always_none(
            len in 0usize..ENVELOPE_SIZE,
        ) {
            let buf = vec![0u8; len];
            prop_assert!(parse_envelope(&buf).is_none());
        }

        // Property: parsing rejects any envelope with an unsupported version.
        #[test]
        fn prop_envelope_wrong_version_always_none(
            version in (0u8..=0xFF).prop_filter("exclude valid version", |v| *v != ENVELOPE_VERSION),
        ) {
            let mut buf = [0u8; ENVELOPE_SIZE];
            buf[0] = version;
            prop_assert!(parse_envelope(&buf).is_none());
        }

        // Property: encoding and then decoding a command payload preserves its contents.
        #[test]
        fn prop_cmd_payload_roundtrip(
            cmd in "[a-zA-Z0-9_]{0,1000}",
            data in prop::collection::vec(0u8..=0xFF, 0..10_000),
        ) {
            let buf = encode_cmd_payload(&cmd, &data);
            let (decoded_cmd, decoded_data) = decode_cmd_payload(&buf)
                .expect("decode must succeed for valid encode output");
            prop_assert_eq!(decoded_cmd, cmd.as_str());
            prop_assert_eq!(decoded_data, &data[..]);
        }

        // Property: the encoded length header always matches the command length.
        #[test]
        fn prop_cmd_payload_length_header_matches_command(
            cmd in "[a-z]{1,500}",
        ) {
            let buf = encode_cmd_payload(&cmd, b"");
            let header = u16::from_le_bytes([buf[0], buf[1]]);
            prop_assert_eq!(header as usize, cmd.len());
        }

        // Property: decoding always fails for inputs shorter than the length header
        #[test]
        fn prop_cmd_payload_decode_short_always_none(
            len in 0usize..2,
        ) {
            let buf = vec![0u8; len];
            prop_assert!(decode_cmd_payload(&buf).is_none());
        }
    }
}

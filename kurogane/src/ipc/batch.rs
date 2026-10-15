//! Batches of IPC messages.
//!
//! Calls the page sends together travel to the browser as one CEF message.
//! Replies travel back the same way. Each item is a `u32` little-endian
//! length, then the envelope and the payload it counts.

use tanso::*;

use crate::ipc::envelope::*;
use crate::ipc::transport::message::{build_message, SHM_THRESHOLD};

/// Size from which a message travels alone and a batch leaves before it
/// takes more.
///
/// 64 KiB keeps batches inline. With 16 calls of 16 KiB in flight on Windows
/// a call took 40 µs in batches up to 64 KiB, 55 µs in batches up to the
/// shared-memory threshold and 62 µs alone.
pub(crate) const BATCH_MAX: usize = if SHM_THRESHOLD < 64 * 1024 {
    SHM_THRESHOLD
} else {
    64 * 1024
};

/// Messages in the order they were pushed.
#[derive(Default)]
pub(crate) struct Batch {
    bytes: Vec<u8>,
    count: u32,
}

impl Batch {
    /// Appends a message whose payload is `parts` in order. Returns false,
    /// leaving the batch as it was, for a message of `BATCH_MAX` bytes or
    /// more, which travels alone.
    pub(crate) fn push(&mut self, envelope: &Envelope, parts: &[&[u8]]) -> bool {
        let len = ENVELOPE_SIZE + parts.iter().map(|part| part.len()).sum::<usize>();
        let Some(prefix) = u32::try_from(len).ok().filter(|_| len < BATCH_MAX) else {
            return false;
        };
        self.bytes.reserve(4 + len);
        self.bytes.extend_from_slice(&prefix.to_le_bytes());
        self.bytes
            .extend_from_slice(&encode_envelope_bytes(envelope));
        for part in parts {
            self.bytes.extend_from_slice(part);
        }
        self.count += 1;
        true
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Returns how many messages the batch holds.
    pub(crate) fn len(&self) -> u32 {
        self.count
    }

    /// Returns whether the batch is long enough to leave before it takes
    /// more.
    pub(crate) fn is_full(&self) -> bool {
        self.bytes.len() >= BATCH_MAX
    }

    /// Builds the message that carries the batch, with `flags` on it. A
    /// batch of one message builds that message as it would travel alone.
    pub(crate) fn build(&self, name: &str, flags: u8) -> Option<ProcessMessage> {
        match self.count {
            0 => None,
            1 => {
                let (mut envelope, payload) = items(&self.bytes).next()?;
                envelope.flags |= flags;
                build_message(name, &envelope, payload)
            }
            count => {
                let envelope = Envelope {
                    version: ENVELOPE_VERSION,
                    subsystem: SUB_RPC,
                    opcode: RPC_BATCH,
                    flags,
                    correlation_id: count,
                    payload_kind: PAYLOAD_EMPTY,
                };
                build_message(name, &envelope, &self.bytes)
            }
        }
    }

    /// Returns the correlation id of the first message.
    pub(crate) fn first_id(&self) -> Option<u32> {
        items(&self.bytes)
            .next()
            .map(|(envelope, _)| envelope.correlation_id)
    }

    /// Builds the message that closes `closes` messages of calls, holding
    /// these answers, none or more.
    pub(crate) fn build_answers(&self, name: &str, closes: u32) -> Option<ProcessMessage> {
        let envelope = Envelope {
            version: ENVELOPE_VERSION,
            subsystem: SUB_RPC,
            opcode: RPC_ANSWERS,
            flags: 0,
            correlation_id: closes,
            payload_kind: PAYLOAD_EMPTY,
        };
        build_message(name, &envelope, &self.bytes)
    }
}

/// Returns the messages of a batch's payload in order, up to the first
/// malformed one.
pub(crate) fn items(payload: &[u8]) -> impl Iterator<Item = (Envelope, &[u8])> {
    let mut rest = payload;
    std::iter::from_fn(move || {
        let len = u32::from_le_bytes(rest.get(..4)?.try_into().ok()?) as usize;
        let item = rest.get(4..)?.get(..len)?;
        rest = &rest[4 + len..];
        parse_envelope(item)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope(id: u32) -> Envelope {
        Envelope {
            version: ENVELOPE_VERSION,
            subsystem: SUB_RPC,
            opcode: RPC_INVOKE,
            flags: 0,
            correlation_id: id,
            payload_kind: PAYLOAD_BINARY,
        }
    }

    fn ids_and_payloads(bytes: &[u8]) -> Vec<(u32, Vec<u8>)> {
        items(bytes)
            .map(|(e, p)| (e.correlation_id, p.to_vec()))
            .collect()
    }

    #[test]
    fn items_come_back_in_order_with_their_payloads() {
        let mut batch = Batch::default();
        assert!(batch.is_empty());
        assert!(batch.push(&envelope(1), &[b"ab", b"c"]));
        assert!(batch.push(&envelope(2), &[]));
        assert!(batch.push(&envelope(3), &[b"xyz"]));
        assert_eq!(
            ids_and_payloads(&batch.bytes),
            vec![(1, b"abc".to_vec()), (2, Vec::new()), (3, b"xyz".to_vec())]
        );
        assert_eq!(batch.count, 3);
    }

    #[test]
    fn a_message_of_batch_max_travels_alone() {
        let mut batch = Batch::default();
        let fits = vec![0u8; BATCH_MAX - ENVELOPE_SIZE - 1];
        let too_long = vec![0u8; BATCH_MAX - ENVELOPE_SIZE];
        assert!(!batch.push(&envelope(1), &[&too_long]));
        assert!(
            batch.is_empty(),
            "a refused message leaves the batch as it was"
        );
        assert!(batch.push(&envelope(2), &[&fits]));
        assert!(batch.is_full(), "a batch near the limit leaves");
    }

    #[test]
    fn reading_stops_at_the_first_malformed_item() {
        let mut batch = Batch::default();
        batch.push(&envelope(1), &[b"one"]);
        batch.push(&envelope(2), &[b"two"]);
        let whole = batch.bytes.clone();

        // A length past the end
        let mut long = whole.clone();
        let second = 4 + ENVELOPE_SIZE + 3;
        long[second..second + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(ids_and_payloads(&long), vec![(1, b"one".to_vec())]);

        // A truncated length and a truncated item
        assert_eq!(ids_and_payloads(&whole[..second + 2]).len(), 1);
        assert_eq!(ids_and_payloads(&whole[..whole.len() - 1]).len(), 1);

        // An item shorter than an envelope
        let mut short = whole[..second].to_vec();
        short.extend_from_slice(&3u32.to_le_bytes());
        short.extend_from_slice(b"abc");
        assert_eq!(ids_and_payloads(&short).len(), 1);

        // Another protocol version
        let mut version = whole.clone();
        version[second + 4] = ENVELOPE_VERSION.wrapping_add(1);
        assert_eq!(ids_and_payloads(&version).len(), 1);

        assert!(items(&[]).next().is_none());
    }
}

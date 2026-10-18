//! Batches of IPC messages.
//!
//! Messages of any subsystem for one frame travel as one CEF message, either
//! way. Each item is a `u32` little-endian length, then the envelope and the
//! payload it counts. The batch's own envelope ([`SUB_BATCH`]) counts the
//! page's messages it closes.

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

/// Returns whether a message whose payload is `parts` joins a batch rather
/// than travelling alone.
pub(crate) fn fits(parts: &[&[u8]]) -> bool {
    ENVELOPE_SIZE + parts.iter().map(|part| part.len()).sum::<usize>() < BATCH_MAX
}

/// Messages in the order they were pushed.
#[derive(Default)]
pub(crate) struct Batch {
    bytes: Vec<u8>,
    count: u32,
}

impl Batch {
    /// Appends a message whose payload is `parts` in order. Returns false,
    /// leaving the batch as it was, for a message that does not [`fits`].
    pub(crate) fn push(&mut self, envelope: &Envelope, parts: &[&[u8]]) -> bool {
        if !fits(parts) {
            return false;
        }
        let len = ENVELOPE_SIZE + parts.iter().map(|part| part.len()).sum::<usize>();
        // Lossless; `fits` bounds it by BATCH_MAX
        let prefix = len as u32;
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

    /// Returns the messages in order.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (Envelope, &[u8])> {
        items(&self.bytes)
    }

    /// Builds the message that carries the batch with `flags` on it, closing
    /// `closes` of the page's messages. One message that closes nothing
    /// travels as itself. `None` for an empty batch that closes nothing, or
    /// when CEF builds no message.
    pub(crate) fn build(&self, flags: u8, closes: u32) -> Option<ProcessMessage> {
        if self.count == 1 && closes == 0 {
            let (mut envelope, payload) = self.iter().next()?;
            envelope.flags |= flags;
            return build_message(&envelope, payload);
        }
        if self.count == 0 && closes == 0 {
            return None;
        }
        let envelope = Envelope {
            version: ENVELOPE_VERSION,
            subsystem: SUB_BATCH,
            opcode: BATCH_ITEMS,
            flags,
            correlation_id: closes,
            payload_kind: PAYLOAD_EMPTY,
        };
        build_message(&envelope, &self.bytes)
    }

    /// Empties the batch, keeping its memory for the next messages.
    pub(crate) fn clear(&mut self) {
        self.bytes.clear();
        self.count = 0;
    }
}

/// Returns the messages of a batch's payload in order, up to the first
/// malformed one.
pub(crate) fn items(payload: &[u8]) -> Items<'_> {
    Items::at(payload, 0)
}

/// Messages of a batch's payload from a byte offset in order, up to the first
/// malformed one.
#[derive(Clone)]
pub(crate) struct Items<'p> {
    payload: &'p [u8],
    pos: usize,
}

impl<'p> Items<'p> {
    /// Messages of `payload` from byte `pos`, where one starts.
    pub(crate) fn at(payload: &'p [u8], pos: usize) -> Self {
        Self { payload, pos }
    }

    /// Returns the offset of the next message.
    pub(crate) fn pos(&self) -> usize {
        self.pos
    }

    /// Returns the next message without taking it.
    pub(crate) fn peek(&self) -> Option<(Envelope, &'p [u8])> {
        self.clone().next()
    }
}

impl<'p> Iterator for Items<'p> {
    type Item = (Envelope, &'p [u8]);

    fn next(&mut self) -> Option<Self::Item> {
        let rest = self.payload.get(self.pos..)?;
        let len = u32::from_le_bytes(rest.get(..4)?.try_into().ok()?) as usize;
        let item = rest.get(4..)?.get(..len)?;
        self.pos += 4 + len;
        parse_envelope(item)
    }
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
        let largest = vec![0u8; BATCH_MAX - ENVELOPE_SIZE - 1];
        let too_long = vec![0u8; BATCH_MAX - ENVELOPE_SIZE];
        assert!(fits(&[&largest]) && !fits(&[&too_long]));
        assert!(!fits(&[&largest, b"x"]), "parts count together");
        assert!(!batch.push(&envelope(1), &[&too_long]));
        assert!(
            batch.is_empty(),
            "a refused message leaves the batch as it was"
        );
        assert!(batch.push(&envelope(2), &[&largest]));
        assert!(batch.is_full(), "a batch near the limit leaves");
    }

    #[test]
    fn a_cleared_batch_starts_over() {
        let mut batch = Batch::default();
        batch.push(&envelope(1), &[b"one"]);
        batch.clear();
        assert!(batch.is_empty() && batch.iter().next().is_none());
        batch.push(&envelope(2), &[b"two"]);
        assert_eq!(ids_and_payloads(&batch.bytes), vec![(2, b"two".to_vec())]);
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

    #[test]
    fn reading_resumes_where_it_stopped() {
        let mut batch = Batch::default();
        for id in 1..=4 {
            batch.push(&envelope(id), &[b"payload"]);
        }
        let mut first = items(&batch.bytes);
        first.next();
        first.next();
        assert_eq!(first.peek().map(|(e, _)| e.correlation_id), Some(3));
        let rest: Vec<u32> = Items::at(&batch.bytes, first.pos())
            .map(|(e, _)| e.correlation_id)
            .collect();
        assert_eq!(rest, [3, 4]);
        let mut done = Items::at(&batch.bytes, batch.bytes.len());
        assert!(done.peek().is_none() && done.next().is_none());
    }
}

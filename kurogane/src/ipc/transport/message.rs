use tetsu::*;

use crate::ipc::envelope::*;
use crate::spec::SandboxMode;

/// Minimum message size for shared-memory transport.
///
/// Smaller messages are sent inline.
pub const SHM_THRESHOLD: usize = if cfg!(target_os = "windows") {
    256 * 1024
} else if cfg!(target_os = "macos") {
    32 * 1024
} else {
    96 * 1024
};

/// A message received over CEF: its envelope and its payload.
///
/// Where the payload lives depends on who sent it. A renderer's message is
/// copied ([`receive_from_renderer`]); the browser's may stay in its
/// shared-memory region and be read in place ([`receive_from_browser`]).
pub struct ReceivedMessage {
    envelope: Envelope,
    bytes: Bytes,
}

/// Private, so that only the receive functions decide which region stays in
/// place: one the browser sent, or an unsandboxed renderer's.
enum Bytes {
    /// Inline ListValue data, or a region copied out of shared memory.
    Owned(Vec<u8>),
    /// A region at least an envelope long, read in place.
    Shared(SharedMemoryRegion),
}

/// Whether the renderers run in Chromium's sandbox, which decides whether
/// the browser copies their shared memory ([`receive_from_renderer`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RendererSandbox {
    /// A renderer is a security boundary: its shared memory is copied.
    Sandboxed,
    /// A compromised renderer already runs with the user's rights: its
    /// shared memory is read in place, as the browser's is.
    Unsandboxed,
}

impl RendererSandbox {
    /// Exhaustive, so a new sandbox mode decides whether it copies.
    pub(crate) fn of(mode: SandboxMode) -> Self {
        match mode {
            SandboxMode::Chromium => Self::Sandboxed,
            SandboxMode::Disabled => Self::Unsandboxed,
        }
    }
}

impl ReceivedMessage {
    pub fn envelope(&self) -> Envelope {
        self.envelope
    }

    pub fn payload(&self) -> &[u8] {
        match &self.bytes {
            Bytes::Owned(payload) => payload,
            Bytes::Shared(region) => {
                // SAFETY: only shared_message makes a Shared, whose callers
                // stand for the region (see receive_from_browser and
                // receive_from_renderer)
                let bytes = unsafe { in_place(region) };
                bytes.get(ENVELOPE_SIZE..).unwrap_or_default()
            }
        }
    }
}

/// The renderer's side: a message from the browser. A shared region is read
/// in place, which saves a copy of every large response and stream chunk the
/// page receives.
///
/// # Safety
///
/// `message` must come from the browser, as every message a renderer
/// receives does. The browser is the only process that wrote the region,
/// and CEF's shared message builder gives up its view when it sends the
/// message, so nothing writes the region while this process reads it.
pub unsafe fn receive_from_browser(message: &ProcessMessage) -> Option<ReceivedMessage> {
    let Some(region) = shared_region(message) else {
        return receive_inline(message);
    };
    // SAFETY: the caller guarantees the browser sent the region
    unsafe { shared_message(region) }
}

/// The browser's side: a message from a renderer.
///
/// A sandboxed renderer's shared region is copied once, here. The renderer
/// is a security boundary then, and a compromised one could keep writing its
/// side of the region (CEF maps it writable in both processes): the copy
/// lets the ACL and the subsystem that runs the message read the same
/// bytes, and nothing in the browser borrows memory another process writes.
///
/// An unsandboxed renderer's region is read in place, as a renderer reads
/// the browser's. An honest renderer gave up its view when it sent it, and a
/// compromised one already runs with the user's rights and could write this
/// process's memory directly: the copy would protect nothing and cost every
/// large upload.
pub fn receive_from_renderer(
    message: &ProcessMessage,
    sandbox: RendererSandbox,
) -> Option<ReceivedMessage> {
    let Some(region) = shared_region(message) else {
        return receive_inline(message);
    };
    match sandbox {
        RendererSandbox::Sandboxed => {
            // SAFETY: CEF keeps the region mapped for `size()` bytes while
            // `region` lives, past this call
            let (envelope, payload) =
                unsafe { copy_shared(region.memory() as *const u8, region.size()) }?;
            Some(ReceivedMessage {
                envelope,
                bytes: Bytes::Owned(payload),
            })
        }
        // SAFETY: deliberately weaker than shared_message asks. Only a
        // compromised renderer could still write the region, and unsandboxed
        // it could already write this process's memory directly
        RendererSandbox::Unsandboxed => unsafe { shared_message(region) },
    }
}

/// A message left in its shared region, to be read in place.
///
/// # Safety
///
/// As for [`in_place`]: no other process may write the region while the
/// message lives.
unsafe fn shared_message(region: SharedMemoryRegion) -> Option<ReceivedMessage> {
    // SAFETY: the caller's guarantee
    let (envelope, _) = parse_envelope(unsafe { in_place(&region) })?;
    Some(ReceivedMessage {
        envelope,
        bytes: Bytes::Shared(region),
    })
}

/// The message's shared-memory region, when it has one that is mapped and
/// holds at least an envelope.
fn shared_region(message: &ProcessMessage) -> Option<SharedMemoryRegion> {
    message.shared_memory_region().filter(|region| {
        region.is_valid() != 0 && region.size() >= ENVELOPE_SIZE && !region.memory().is_null()
    })
}

/// The bytes of a shared region, in place.
///
/// # Safety
///
/// No other process may write the region while the returned slice lives:
/// it must be one the browser sent (see [`receive_from_browser`]). A
/// sandboxed renderer's region is copied instead ([`copy_shared`]).
unsafe fn in_place(region: &SharedMemoryRegion) -> &[u8] {
    let base = region.memory() as *const u8;
    if base.is_null() {
        return &[];
    }
    // SAFETY: `region` keeps `size()` bytes mapped while it lives, which the
    // slice cannot outlive; the caller guarantees that nothing writes them
    unsafe { std::slice::from_raw_parts(base, region.size()) }
}

/// Copies a shared-memory message out of its region: the envelope, then the
/// payload.
///
/// # Safety
///
/// `base` must be readable for `len` bytes for the duration of the call.
/// Another process may write them meanwhile: they are only copied, by raw
/// pointer, and never borrowed as a slice.
unsafe fn copy_shared(base: *const u8, len: usize) -> Option<(Envelope, Vec<u8>)> {
    if base.is_null() || len < ENVELOPE_SIZE {
        return None;
    }
    let mut header = [0u8; ENVELOPE_SIZE];
    // SAFETY: the caller guarantees `len` readable bytes at `base`, and
    // `header` is a buffer of its own. A sender still writing makes this a
    // racy read of plain bytes, which Chromium also accepts for shared
    // memory: copy first, then validate the copy
    unsafe { std::ptr::copy_nonoverlapping(base, header.as_mut_ptr(), ENVELOPE_SIZE) };
    // Validated before the payload is copied: a malformed message costs no copy
    let (envelope, _) = parse_envelope(&header)?;
    let size = len - ENVELOPE_SIZE;
    let mut payload = Vec::with_capacity(size);
    // SAFETY: as above for the source; `payload` has room for `size` bytes,
    // which the copy initializes before set_len exposes them. The buffer is
    // not zeroed first: every byte is written by the copy
    unsafe {
        std::ptr::copy_nonoverlapping(base.add(ENVELOPE_SIZE), payload.as_mut_ptr(), size);
        payload.set_len(size);
    }
    Some((envelope, payload))
}

/// Builds a ProcessMessage from an envelope and payload.
///
/// Uses shared-memory transport for large messages and falls back to
/// inline transport if shared memory is unavailable.
pub fn build_message(name: &str, envelope: &Envelope, payload: &[u8]) -> Option<ProcessMessage> {
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
        build_shm_parts(name, envelope, parts).or_else(|| build_inline_parts(name, envelope, parts))
    }
}

/// Build an inline ProcessMessage using CEF ListValue fields.
///
/// Envelope fields are stored as individual ListValue entries to avoid
/// an extra flat-buffer serialization. CEF's native IPC transports
/// the structured ListValue directly.
fn build_inline_parts(name: &str, envelope: &Envelope, parts: &[&[u8]]) -> Option<ProcessMessage> {
    let msg = process_message_create(Some(&CefString::from(name)))?;
    let args = msg.argument_list()?;

    args.set_int(0, envelope.version.into());
    args.set_int(1, envelope.subsystem.into());
    args.set_int(2, envelope.opcode.into());
    args.set_int(3, envelope.flags.into());
    // A ListValue holds only i32; the id travels as its bit pattern
    args.set_int(4, envelope.correlation_id as i32);
    args.set_int(5, envelope.payload_kind.into());

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

fn build_shm_parts(name: &str, envelope: &Envelope, parts: &[&[u8]]) -> Option<ProcessMessage> {
    let total_payload: usize = parts.iter().map(|p| p.len()).sum();
    let total_size = ENVELOPE_SIZE + total_payload;
    let builder = shared_process_message_builder_create(Some(&CefString::from(name)), total_size)?;
    if builder.is_valid() == 0 {
        return None;
    }

    // SAFETY: `builder.memory()` remains valid and writable for `total_size`
    // bytes throughout `builder`'s lifetime. Total written bytes (`ENVELOPE_SIZE`
    // plus payload parts) equal `total_size` keeping all offsets strictly in bounds.
    // Sources are distinct Rust buffers outside the newly mapped region, satisfying
    // `copy_nonoverlapping`.
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

/// A message sent inline, in CEF ListValue fields.
fn receive_inline(message: &ProcessMessage) -> Option<ReceivedMessage> {
    let args = message.argument_list()?;

    // A byte field out of range is malformed, not truncated into another value
    let byte = |index| u8::try_from(args.int(index)).ok();

    let envelope = Envelope {
        version: byte(0)?,
        subsystem: byte(1)?,
        opcode: byte(2)?,
        flags: byte(3)?,
        correlation_id: args.int(4) as u32,
        payload_kind: byte(5)?,
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

    Some(ReceivedMessage {
        envelope,
        bytes: Bytes::Owned(payload),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope() -> Envelope {
        Envelope {
            version: ENVELOPE_VERSION,
            subsystem: 1,
            opcode: 2,
            flags: 0,
            correlation_id: 7,
            payload_kind: 0,
        }
    }

    fn copied(region: &[u8]) -> Option<(Envelope, Vec<u8>)> {
        // SAFETY: `region` is readable for its length during the call.
        unsafe { copy_shared(region.as_ptr(), region.len()) }
    }

    #[test]
    fn decode_shm_rejects_wrong_version() {
        let mut region = encode_envelope_bytes(&envelope()).to_vec();
        region.extend_from_slice(b"payload");
        assert_eq!(copied(&region).map(|(e, _)| e.correlation_id), Some(7));
        for version in [0, ENVELOPE_VERSION.wrapping_add(1), 0xFF] {
            region[0] = version;
            assert!(copied(&region).is_none(), "version {version} was accepted");
        }
        assert!(copied(&region[..ENVELOPE_SIZE - 1]).is_none());
        assert!(copied(&[]).is_none());
        // SAFETY: a null pointer is refused before it is read.
        assert!(unsafe { copy_shared(std::ptr::null(), 64) }.is_none());
    }
}

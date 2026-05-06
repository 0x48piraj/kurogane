//! Shared memory helper for binary IPC.

use shared_memory::{Shmem, ShmemConf};

// Empirically derived crossover point (~2.5-3MB) where SHM becomes faster than inline
pub const SHM_THRESHOLD: usize = 3 * 1024 * 1024; // 3MB

pub const SHM_HEADER_SIZE: usize = 4;
pub const MAX_SHM_SIZE: usize = 64 * 1024 * 1024; // 64MB

pub struct SharedBuffer {
    shmem: Shmem,
    size: usize, // total mapping size (header + payload)
}

// SAFETY: On Windows, the shared_memory crate exposes a raw OS handle
// ViewOfFile(*mut c_void) that does not implement Send by default.
// In practice, this handle is safe to use from any thread because the OS
// allows it. We wrap access in a Mutex<HashMap> to enforce exclusive
// access and ensure thread safety. This makes it sound to mark the wrapper
// as Send.
unsafe impl Send for SharedBuffer {}

impl SharedBuffer {

    /// Create a new framed shared memory region.
    pub fn create(payload_size: usize) -> Result<Self, String> {
        let total_size = SHM_HEADER_SIZE + payload_size;

        let shmem = ShmemConf::new()
            .size(total_size)
            .create()
            .map_err(|e| format!("shm create failed: {}", e))?;

        Ok(Self { shmem, size: total_size })
    }

    /// Open an existing shared memory region.
    /// Size is advisory only.
    pub fn open(name: &str, expected_size: usize) -> Result<Self, String> {
        let shmem = ShmemConf::new()
            .os_id(name)
            .open()
            .map_err(|e| format!("shm open '{}': {}", name, e))?;

        let actual_size = shmem.len();

        // Shared memory mappings may be larger than requested due to OS page/alignment.
        // Only enforce a lower bound; equality is not guaranteed.
        if actual_size < expected_size {
            return Err(format!(
                "SHM smaller than expected: expected={} actual={}",
                expected_size,
                actual_size
            ));
        }

        Ok(Self {
            shmem,
            size: expected_size, // protocol boundary, not actual mapped size
        })
    }

    /// OS identifier used for cross-process sharing.
    pub fn name(&self) -> String {
        self.shmem.get_os_id().to_string()
    }

    /// Copy data into the shared memory.
    pub fn write(&mut self, data: &[u8]) -> Result<(), String> {
        if SHM_HEADER_SIZE + data.len() > self.size {
            return Err(format!(
                "SHM overflow: payload={} total_capacity={}",
                data.len(),
                self.size
            ));
        }

        unsafe {
            let ptr = self.shmem.as_ptr();
            let slice = std::slice::from_raw_parts_mut(ptr, self.size);

            // Write header
            let len_bytes = (data.len() as u32).to_le_bytes();
            slice[0..4].copy_from_slice(&len_bytes);

            // Write payload
            slice[4..4 + data.len()].copy_from_slice(data);
        }

        Ok(())
    }

    /// Read payload safely-ish (validated via header)
    ///
    /// SHM is written completely before the sender publishes the
    /// corresponding CEF IPC message.
    ///
    /// The receiver only reads SHM after receiving that message.
    ///
    /// Therefore IPC delivery itself acts as the synchronization
    /// boundary for visibility of the shared memory contents.
    ///
    /// The returned slice is scoped to this closure to prevent
    /// retention beyond the synchronous read boundary.
    pub fn with_read<R>(
        &self,
        f: impl FnOnce(&[u8]) -> R,
    ) -> Result<R, String> {
        if self.size < SHM_HEADER_SIZE {
            return Err("SHM too small for header".into());
        }

        unsafe {
            let ptr = self.shmem.as_ptr();

            let mut len_bytes = [0u8; 4];

            std::ptr::copy_nonoverlapping(
                ptr,
                len_bytes.as_mut_ptr(),
                SHM_HEADER_SIZE,
            );

            let payload_len = u32::from_le_bytes(len_bytes) as usize;

            if payload_len > self.size - SHM_HEADER_SIZE {
                return Err(format!(
                    "Corrupted SHM: payload_len={} > available={}",
                    payload_len,
                    self.size - SHM_HEADER_SIZE
                ));
            }

            let payload_ptr = ptr.add(SHM_HEADER_SIZE);

            let payload =
                std::slice::from_raw_parts(payload_ptr, payload_len);

            Ok(f(payload))
        }
    }
}

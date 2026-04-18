//! Shared memory helper for binary IPC.

use shared_memory::{Shmem, ShmemConf};

// Empirically derived crossover point (~2.5-3MB) where SHM becomes faster than inline
pub const SHM_THRESHOLD: usize = 3 * 1024 * 1024; // 3MB

pub const SHM_HEADER_SIZE: usize = 4;

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

        // We still store expected_size, but will validate against header
        Ok(Self {
            shmem,
            size: expected_size,
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

    /// Read payload safely (validated via header)
    pub fn read(&self) -> Result<&[u8], String> {
        unsafe {
            let ptr = self.shmem.as_ptr();
            let slice = std::slice::from_raw_parts(ptr, self.size);

            if slice.len() < SHM_HEADER_SIZE {
                return Err("SHM too small for header".into());
            }

            let mut len_bytes = [0u8; 4];
            len_bytes.copy_from_slice(&slice[0..4]);

            let payload_len = u32::from_le_bytes(len_bytes) as usize;

            if payload_len > slice.len() - SHM_HEADER_SIZE {
                return Err(format!(
                    "Corrupted SHM: payload_len={} > available={}",
                    payload_len,
                    slice.len() - SHM_HEADER_SIZE
                ));
            }

            Ok(&slice[4..4 + payload_len])
        }
    }
}

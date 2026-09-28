use std::sync::Arc;

use cef::{ImplSharedMemoryRegion, SharedMemoryRegion};

pub trait BinaryBuffer: Send + Sync {
    fn data(&self) -> &[u8];
}

pub struct ShmBinary {
    region: SharedMemoryRegion,
    offset: usize,
}

impl ShmBinary {
    pub fn new(region: SharedMemoryRegion, offset: usize) -> Self {
        Self { region, offset }
    }
}

impl BinaryBuffer for ShmBinary {
    fn data(&self) -> &[u8] {
        if self.region.is_valid() == 0 || self.offset >= self.region.size() {
            return &[];
        }

        // SAFETY: `region` is valid for `size()` bytes and remains mapped for the
        // lifetime of the borrow. `offset < size()` keeps the slice within the mapping.
        // Untrusted IPC writes from a rogue renderer may corrupt byte payload but
        // cannot breach slice boundaries or cause structural memory safety UB.
        unsafe {
            let ptr = self.region.memory() as *const u8;
            std::slice::from_raw_parts(ptr.add(self.offset), self.region.size() - self.offset)
        }
    }
}

/// Clones the shared memory region.
impl Clone for ShmBinary {
    fn clone(&self) -> Self {
        Self {
            region: self.region.clone(),
            offset: self.offset,
        }
    }
}

pub type SharedBinary = Arc<dyn BinaryBuffer>;

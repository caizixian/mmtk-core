//! Mark sweep space.
//! MMTk provides two implementations of mark sweep:
//! 1. mark sweep using a native freelist allocator implemented in MMTk. This is the default mark sweep implementation, and
//!    most people should use this.
//! 2. mark sweep using malloc as its freelist allocator. Use the feature `malloc_mark_sweep` to enable it. As we do not control
//!    the allocation of malloc, we have to work around a few issues to make it for mark sweep. Thus it has considerably worse performance.
//!    This is an experimental feature, and should only be used if you are actually interested in using malloc as the allocator.
//!    Otherwise this should not be used.

// TODO: we should extract the code about mark sweep, and make both implementation use the same mark sweep code.

use std::collections::HashSet;
use std::sync::Mutex;

use crate::util::heap::chunk_map::Chunk;

/// RAII lease ensuring at most one GC worker sweeps a given `chunk` in a mark-sweep space.
pub(super) struct ChunkSweepLease<'a> {
    sweeping_chunks: &'a Mutex<HashSet<Chunk>>,
    chunk: Chunk,
}

impl<'a> ChunkSweepLease<'a> {
    pub(super) fn acquire(
        sweeping_chunks: &'a Mutex<HashSet<Chunk>>,
        chunk: Chunk,
        space_name: &'static str,
    ) -> Self {
        let inserted = sweeping_chunks.lock().unwrap().insert(chunk);
        assert!(
            inserted,
            "Concurrent sweep attempted on {space_name} chunk {chunk:?}"
        );
        Self {
            sweeping_chunks,
            chunk,
        }
    }
}

impl Drop for ChunkSweepLease<'_> {
    fn drop(&mut self) {
        self.sweeping_chunks.lock().unwrap().remove(&self.chunk);
    }
}

/// Malloc mark sweep. This uses `MallocSpace` and `MallocAllocator`.
#[cfg_attr(not(feature = "malloc_mark_sweep"), allow(dead_code))]
pub(crate) mod malloc_ms;
/// Native mark sweep. This uses `MarkSweepSpace` and `FreeListAllocator`.
#[cfg_attr(
    all(feature = "malloc_mark_sweep", not(feature = "marksweep_as_nonmoving")),
    allow(dead_code)
)]
pub(crate) mod native_ms;

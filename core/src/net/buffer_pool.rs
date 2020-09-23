use crate::net::buffer::{BufferChunk, BufferConfig, Chunk, DefaultChunk};
use std::{
    collections::{vec_deque::Drain, VecDeque},
    fmt::Debug,
    sync::Arc,
};

/// Methods required by a ChunkAllocator
pub trait ChunkAllocator: Send + Sync + Debug + 'static {
    /// ChunkAllocators deliver Chunk by raw pointers
    fn get_chunk(&self) -> *mut dyn Chunk;
    /// This method tells the allocator that the [Chunk](Chunk) may be deallocated
    ///
    /// # Safety
    ///
    /// The caller *must* guarantee that the memory pointed to by `ptr`
    /// has *not* been deallocated, yet!
    unsafe fn release(&self, ptr: *mut dyn Chunk) -> ();
}

/// A default allocator for Kompact
///
/// Heap allocates chunks through the normal Rust system allocator
#[derive(Default, Debug)]
pub(crate) struct DefaultAllocator {
    chunk_size: usize,
}

impl ChunkAllocator for DefaultAllocator {
    fn get_chunk(&self) -> *mut dyn Chunk {
        Box::into_raw(Box::new(DefaultChunk::new(self.chunk_size)))
    }

    unsafe fn release(&self, ptr: *mut dyn Chunk) -> () {
        Box::from_raw(ptr);
    }
}

pub(crate) struct BufferPool {
    pool: VecDeque<BufferChunk>,
    /// Counts the number of BufferChunks allocated by this pool
    pool_size: usize,
    chunk_allocator: Arc<dyn ChunkAllocator>,
    max_pool_size: usize,
}

impl BufferPool {
    /// Creates a BufferPool from a BufferConfig
    pub fn with_config(
        config: &BufferConfig,
        custom_allocator: &Option<Arc<dyn ChunkAllocator>>,
    ) -> Self {
        config.validate();
        let chunk_allocator = {
            if let Some(allocator) = custom_allocator {
                allocator.clone()
            } else {
                Arc::new(DefaultAllocator {
                    chunk_size: config.chunk_size,
                })
            }
        };
        let mut pool = VecDeque::<BufferChunk>::new();
        for _ in 0..config.initial_pool_count {
            pool.push_front(BufferChunk::from_chunk(chunk_allocator.get_chunk()));
        }
        BufferPool {
            pool,
            pool_size: config.initial_pool_count,
            chunk_allocator,
            max_pool_size: config.max_pool_count,
        }
    }

    fn new_buffer(&self) -> BufferChunk {
        BufferChunk::from_chunk(self.chunk_allocator.get_chunk())
    }

    pub fn get_buffer(&mut self) -> Option<BufferChunk> {
        self.try_reclaim()
    }

    pub fn return_buffer(&mut self, mut buffer: BufferChunk) -> () {
        buffer.lock();
        self.pool.push_back(buffer);
    }

    pub fn drain_returned(&mut self) -> Drain<BufferChunk> {
        self.pool.drain(0..)
    }

    /// Iterates of returned buffers from oldest to newest trying to reclaim
    /// until it successfully finds an available one
    /// If it fails it will attempt to create a new buffer instead
    fn try_reclaim(&mut self) -> Option<BufferChunk> {
        for _i in 0..self.pool.len() {
            if let Some(mut buffer) = self.pool.pop_front() {
                if buffer.free() {
                    return Some(buffer);
                } else {
                    self.pool.push_back(buffer);
                }
            }
        }
        self.increase_pool()
    }

    fn increase_pool(&mut self) -> Option<BufferChunk> {
        if self.pool_size >= self.max_pool_size {
            return None;
        };
        self.pool_size += 1;
        Some(self.new_buffer())
    }

    /// We use this method for assertions in tests
    #[allow(dead_code)]
    pub(crate) fn get_pool_sizes(&self) -> (usize, usize) {
        (self.pool_size, self.pool.len())
    }
}

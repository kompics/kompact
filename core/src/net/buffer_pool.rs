use crate::net::buffer::{BufferChunk, Chunk, DefaultChunk};
use std::collections::VecDeque;
use std::collections::vec_deque::Drain;

/// The number of Buffers each pool will Pre-allocate
pub const INITIAL_BUFFER_LEN: usize = 5;
const MAX_POOL_SIZE: usize = 10000;

/// Methods required by a ChunkAllocator
pub trait ChunkAllocator: Send + 'static {
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
#[derive(Default)]
pub(crate) struct DefaultAllocator {}

impl ChunkAllocator for DefaultAllocator {
    fn get_chunk(&self) -> *mut dyn Chunk {
        Box::into_raw(Box::new(DefaultChunk::new()))
    }

    unsafe fn release(&self, ptr: *mut dyn Chunk) -> () {
        Box::from_raw(ptr);
    }
}

pub(crate) struct BufferPool {
    pool: VecDeque<BufferChunk>,
    returned: VecDeque<BufferChunk>,
    pool_size: usize,
    chunk_allocator: Box<dyn ChunkAllocator>,
}

impl BufferPool {
    pub fn new() -> Self {
        let mut pool = VecDeque::<BufferChunk>::new();
        let chunk_allocator = Box::new(DefaultAllocator::default());
        for _ in 0..INITIAL_BUFFER_LEN {
            pool.push_front(BufferChunk::from_chunk(chunk_allocator.get_chunk()));
        }
        BufferPool {
            pool,
            returned: VecDeque::new(),
            pool_size: INITIAL_BUFFER_LEN,
            chunk_allocator,
        }
    }

    /// Creates a BufferPool with a custom ChunkAllocator
    #[allow(dead_code)]
    pub fn with_allocator(chunk_allocator: Box<dyn ChunkAllocator>) -> Self {
        let mut pool = VecDeque::<BufferChunk>::new();
        for _ in 0..INITIAL_BUFFER_LEN {
            pool.push_front(BufferChunk::from_chunk(chunk_allocator.get_chunk()));
        }
        BufferPool {
            pool,
            returned: VecDeque::new(),
            pool_size: INITIAL_BUFFER_LEN,
            chunk_allocator,
        }
    }

    fn new_buffer(&self) -> BufferChunk {
        BufferChunk::from_chunk(self.chunk_allocator.get_chunk())
    }

    pub fn get_buffer(&mut self) -> Option<BufferChunk> {
        if let Some(new_buffer) = self.pool.pop_front() {
            return Some(new_buffer);
        }
        self.try_reclaim()
    }

    pub fn return_buffer(&mut self, buffer: BufferChunk) -> () {
        self.returned.push_back(buffer);
    }

    pub fn drain_returned(&mut self) -> Drain<BufferChunk> {
        self.returned.drain(0..)
    }

    /// Iterates of returned buffers from oldest to newest trying to reclaim
    /// until it successfully finds an available one
    /// If it fails it will attempt to create a new buffer instead
    fn try_reclaim(&mut self) -> Option<BufferChunk> {
        for _i in 0..self.returned.len() {
            if let Some(mut returned_buffer) = self.returned.pop_front() {
                if returned_buffer.free() {
                    return Some(returned_buffer);
                } else {
                    self.returned.push_back(returned_buffer);
                }
            }
        }
        self.increase_pool()
    }

    fn increase_pool(&mut self) -> Option<BufferChunk> {
        if self.pool_size >= MAX_POOL_SIZE {
            return None;
        };
        self.pool_size += 1;
        Some(self.new_buffer())
    }

    /// We use this method for assertions in tests
    #[allow(dead_code)]
    pub(crate) fn get_pool_sizes(&self) -> (usize, usize, usize) {
        (self.pool_size, self.pool.len(), self.returned.len())
    }
}

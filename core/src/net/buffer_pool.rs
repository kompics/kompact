use crate::net::{
    buffer::{BufferChunk, Chunk, DefaultChunk},
    frames,
    frames::*,
};
use bytes::{Buf, BufMut, BytesMut};
use iovec::IoVec;
use std::collections::VecDeque;

pub const FRAME_HEAD_LEN: usize = frames::FRAME_HEAD_LEN as usize;
const INITIAL_BUFFER_LEN: usize = 5;
const MAX_POOL_SIZE: usize = 10000;

/// Methods required by a ChunkAllocator
pub trait ChunkAllocator: Send + 'static {
    fn get_chunk(&self) -> Box<dyn Chunk>;
}

/// A default allocator for Kompact
///
/// Heap allocates chunks through the normal Rust system allocator
#[derive(Default)]
pub(crate) struct DefaultAllocator {}
impl ChunkAllocator for DefaultAllocator {
    fn get_chunk(&self) -> Box<dyn Chunk> {
        Box::new(DefaultChunk::new())
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

    /// Iterates of returned buffers from oldest to newest trying to reclaim
    /// until it successfully finds an available one
    /// If it fails it will attempt to create a new buffer instead
    fn try_reclaim(&mut self) -> Option<BufferChunk> {
        for i in 0..self.returned.len() {
            if let Some(mut returned_buffer) = self.returned.pop_front() {
                if returned_buffer.free() {
                    //println!("Reclaimed a buffer!");
                    return Some(returned_buffer);
                } else {
                    //println!("Buffer reclaim attempt failed!");
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
        //println!("Increased pool size to {}", self.pool_size);
        Some(self.new_buffer())
    }
}

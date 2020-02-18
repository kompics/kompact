use bytes::{BytesMut, BufMut, Buf};
use iovec::IoVec;
use crate::net::frames::*;
use crate::net::frames;
use crate::net::buffer::BufferChunk;
use std::collections::VecDeque;

pub const FRAME_HEAD_LEN: usize = frames::FRAME_HEAD_LEN as usize;
const INITIAL_BUFFER_LEN: usize = 5;
const MAX_POOL_SIZE: usize = 10000;

pub(crate) struct BufferPool {
    pool: VecDeque<BufferChunk>,
    returned: VecDeque<BufferChunk>,
    pool_size: usize,
}

impl BufferPool {
    pub fn new() -> Self {
        let mut pool = VecDeque::<BufferChunk>::new();
        let mut returned= VecDeque::<BufferChunk>::new();
        let mut pool_size = 0;
        for i in 0..INITIAL_BUFFER_LEN {
            pool_size += 1;
            pool.push_front(Self::new_buffer())
        }
        BufferPool{
            pool,
            returned,
            pool_size,
        }
    }

    fn new_buffer() -> BufferChunk {
        BufferChunk::new()
    }

    pub fn get_buffer(&mut self) -> Option<BufferChunk> {
        if let Some(new_buffer) = self.pool.pop_front() {
            return Some(new_buffer)
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
                    return Some(returned_buffer)
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
            return None
        };
        self.pool_size += 1;
        //println!("Increased pool size to {}", self.pool_size);
        Some(Self::new_buffer())
    }
}
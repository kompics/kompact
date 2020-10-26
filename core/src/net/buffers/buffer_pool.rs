use crate::net::buffers::{BufferChunk, BufferConfig, ChunkAllocator, DefaultAllocator};
use std::{
    collections::{vec_deque::Drain, VecDeque},
    sync::Arc,
};

pub(crate) struct BufferPool {
    pool: VecDeque<BufferChunk>,
    // Counts the number of BufferChunks allocated by this pool
    pool_size: usize,
    chunk_allocator: Arc<dyn ChunkAllocator>,
    max_pool_size: usize,
}

impl BufferPool {
    /// Creates a `BufferPool` from a BufferConfig and an *optional* [ChunkAllocator]
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
        for _ in 0..config.initial_chunk_count {
            pool.push_front(BufferChunk::from_chunk(chunk_allocator.get_chunk()));
        }
        BufferPool {
            pool,
            pool_size: config.initial_chunk_count,
            chunk_allocator,
            max_pool_size: config.max_chunk_count,
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

    /// Returns the number of allocated buffers and the current number of buffers in the pool
    #[allow(dead_code)]
    pub(crate) fn get_pool_sizes(&self) -> (usize, usize) {
        (self.pool_size, self.pool.len())
    }

    /// Counts the number of locked chunks currently in the pool
    #[allow(dead_code)]
    pub(crate) fn count_locked_chunks(&mut self) -> usize {
        let mut cnt = 0;
        for buffer in &mut self.pool {
            if !buffer.free() {
                cnt += 1;
            }
        }
        cnt
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_pool_limit_and_size() {
        let mut cfg = BufferConfig::default();
        cfg.initial_chunk_count(2);
        cfg.max_chunk_count(5);
        cfg.chunk_size(128);

        let mut pool = BufferPool::with_config(&cfg, &None);

        assert_eq!(pool.get_buffer().unwrap().len(), 128);
        assert!(pool.get_buffer().is_some());
        assert_eq!(pool.pool_size, 2);
        assert!(pool.get_buffer().is_some());
        assert_eq!(pool.pool_size, 3);
        assert!(pool.get_buffer().is_some());
        assert_eq!(pool.pool_size, 4);
        assert_eq!(pool.get_buffer().unwrap().len(), 128);
        assert_eq!(pool.pool_size, 5);
        assert!(pool.get_buffer().is_none());
        assert_eq!(pool.pool_size, 5);
        // None have been returned
        assert_eq!(pool.count_locked_chunks(), 0);
    }

    #[test]
    fn buffer_pool_return_and_allocated_and_unlock() {
        let mut cfg = BufferConfig::default();
        cfg.initial_chunk_count(1);
        cfg.max_chunk_count(5);
        cfg.chunk_size(128);

        let mut pool = BufferPool::with_config(&cfg, &None);
        {
            let buf1 = pool.get_buffer().unwrap();
            let _lock = buf1.get_lock();
            pool.return_buffer(buf1);
            assert_eq!(pool.count_locked_chunks(), 1);
            assert_eq!(pool.pool_size, 1);
            let buf2 = pool.get_buffer().unwrap();
            assert_eq!(pool.pool_size, 2);
            pool.return_buffer(buf2);
            let buf3 = pool.get_buffer().unwrap();
            assert_eq!(pool.pool_size, 2); // buf2 reused
            let buf4 = pool.get_buffer().unwrap();
            assert_eq!(pool.pool_size, 3); // buf allocated, buf1 still locked
            pool.return_buffer(buf3);
            pool.return_buffer(buf4);
        }
        let _buf5 = pool.get_buffer().unwrap();
        assert_eq!(pool.pool_size, 3);
        let _buf6 = pool.get_buffer().unwrap();
        assert_eq!(pool.pool_size, 3);
        let _buf7 = pool.get_buffer().unwrap();
        assert_eq!(pool.pool_size, 3);
        let _buf8 = pool.get_buffer().unwrap();
        assert_eq!(pool.pool_size, 4);
        assert_eq!(pool.count_locked_chunks(), 0);
    }
}

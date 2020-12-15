use super::*;
use bytes::buf::UninitSlice;

/// Structure used by components to continuously extract leases from BufferChunks swapped with the internal/private BufferPool
/// Using get_buffer_encoder() we get a BufferEncoder which implements BufMut interface for the underlying Chunks
/// The BufferEncoder uses its lifetime and Drop impl to give compile-time safety for consecutive encoding attempts
pub struct EncodeBuffer {
    buffer: BufferChunk,
    pub(crate) buffer_pool: BufferPool,
    write_offset: usize,
    read_offset: usize,
    dispatcher_ref: Option<DispatcherRef>,
    pub(crate) min_remaining: usize,
}

impl EncodeBuffer {
    /// Creates a new EncodeBuffer, allocates a new BufferPool.
    pub fn with_config(
        config: &BufferConfig,
        custom_allocator: &Option<Arc<dyn ChunkAllocator>>,
    ) -> Self {
        let mut buffer_pool = BufferPool::with_config(config, custom_allocator);
        if let Some(buffer) = buffer_pool.get_buffer() {
            EncodeBuffer {
                buffer,
                buffer_pool,
                write_offset: 0,
                read_offset: 0,
                dispatcher_ref: None,
                min_remaining: config.encode_buf_min_free_space,
            }
        } else {
            panic!(
                "Couldn't initialize EncodeBuffer, bad config: {:?}",
                &config
            );
        }
    }

    /// Creates a new EncodeBuffer with a DispatcherRef used for safe destruction of the buffers.
    pub fn with_dispatcher_ref(
        dispatcher_ref: DispatcherRef,
        config: &BufferConfig,
        custom_allocator: Option<Arc<dyn ChunkAllocator>>,
    ) -> Self {
        let mut buffer_pool = BufferPool::with_config(config, &custom_allocator);
        if let Some(buffer) = buffer_pool.get_buffer() {
            EncodeBuffer {
                buffer,
                buffer_pool,
                write_offset: 0,
                read_offset: 0,
                dispatcher_ref: Some(dispatcher_ref),
                min_remaining: config.encode_buf_min_free_space,
            }
        } else {
            panic!(
                "Couldn't initialize EncodeBuffer, bad config: {:?}",
                &config
            );
        }
    }

    /// Returns a `BufferEncoder` which allows for encoding into the Buffer
    pub fn get_buffer_encoder(&mut self) -> Result<BufferEncoder, SerError> {
        if (self.buffer.len() - self.write_offset) < self.min_remaining {
            // Eagerly swap to avoid unnecessary chaining
            self.swap_buffer()?;
        }
        Ok(BufferEncoder::new(self))
    }

    /// Swap the current buffer with a fresh one from the private buffer_pool
    pub fn swap_buffer(&mut self) -> Result<(), SerError> {
        if let Some(mut new_buffer) = self.buffer_pool.get_buffer() {
            self.buffer.swap_buffer(&mut new_buffer);
            self.read_offset = 0;
            self.write_offset = 0;
            // new_buffer has been swapped in-place, return it to the pool
            new_buffer.lock();
            self.buffer_pool.return_buffer(new_buffer);
            Ok(())
        } else {
            Err(SerError::NoBuffersAvailable(
                "No Available Buffers in BufferPool".to_string(),
            ))
        }
    }

    /// Returns the remaining writeable length of the current `BufferChunk`
    pub fn writeable_len(&self) -> usize {
        self.buffer.len() - self.write_offset
    }

    /// Extracts the bytes between the read-pointer and the write-pointer and advances the read-pointer
    /// Ensures there's a configurable-minimum left in the current buffer, minimizes overflow during swap
    fn get_chunk_lease(&mut self) -> Option<ChunkLease> {
        let cnt = self.write_offset - self.read_offset;
        if cnt > 0 {
            self.read_offset += cnt;
            let lease = self
                .buffer
                .get_lease(self.write_offset - cnt, self.write_offset);

            return Some(lease);
        }
        None
    }

    pub(crate) fn get_write_offset(&self) -> usize {
        self.write_offset
    }

    pub(crate) fn get_read_offset(&self) -> usize {
        self.read_offset
    }

    /// Returns the full length of the current buffer `BufferChunk`
    #[allow(dead_code)]
    pub(crate) fn len(&self) -> usize {
        self.buffer.len()
    }
}

impl Drop for EncodeBuffer {
    fn drop(&mut self) {
        // If the buffer was spawned with a dispatcher_ref all locked buffers are sent to the dispatcher for garbage collection
        if let Some(dispatcher_ref) = self.dispatcher_ref.take() {
            // Drain the returned queue and send locked buffers to the dispatcher
            for mut trash in self.buffer_pool.drain_returned() {
                if !trash.free() {
                    dispatcher_ref.tell(DispatchEnvelope::LockedChunk(trash));
                }
            }
            // Make sure that the current buffer is cleaned up safely
            self.buffer.lock();
            if !self.buffer.free() {
                // Create an empty dummy buff so we can swap self.buffer for garbage collection.
                let mut dummy_buff = BufferChunk::new(1);
                self.buffer.swap_buffer(&mut dummy_buff);
                dispatcher_ref.tell(DispatchEnvelope::LockedChunk(dummy_buff));
            }
        }
    }
}

/// Instantiated Writer/Encoder for EncodeBuffer
/// Provides the BufMut interface over an EnodeBuffer ensuring that the EncodeBuffer will be aligned
/// after the BufferEncoder has been dropped.
pub struct BufferEncoder<'a> {
    encode_buffer: &'a mut EncodeBuffer,
    chain_head: Option<ChunkLease>,
    ser_error: Option<SerError>,
}

impl<'a> BufferEncoder<'a> {
    fn new(encode_buffer: &'a mut EncodeBuffer) -> Self {
        // Check alignment, make sure we start aligned:
        assert_eq!(
            encode_buffer.get_write_offset(),
            encode_buffer.get_read_offset()
        );
        BufferEncoder {
            encode_buffer,
            chain_head: None,
            ser_error: None,
        }
    }

    /// This method may perform an eager-swap of the underlying buffer to avoid chaining ChunkLeases.
    ///
    /// Also ensures that there are available buffers to fit the entire message into.
    /// Returns `SerError::NoBuffersAvailable` error if there are no available buffers to fit the
    /// entire message into.
    pub(crate) fn try_reserve(&mut self, size: usize) -> Result<(), SerError> {
        // Length of BufferChunks in this encode_buffer
        let buf_len = self.encode_buffer.len();
        // Remaining space in the current BufferChunk
        let remaining = buf_len - self.encode_buffer.get_write_offset();

        if remaining > size {
            Ok(())
        } else if size < buf_len {
            // Do an eager swap and propagate the result
            self.chain_and_swap()
        } else {
            // Reserve additional buffers in the pool but dont swap now.
            self.encode_buffer
                .buffer_pool
                .try_reserve(size - remaining)
                .map_err(|e| SerError::NoBuffersAvailable(e.to_string()))
        }
    }

    pub(crate) fn pad(&mut self, cnt: usize) {
        unsafe {
            self.advance_mut(cnt);
        }
    }

    /// Returns everything (if any) written into the BufferEncoder as a ChunkLease
    /// Used when serialisation is complete
    pub fn get_chunk_lease(&mut self) -> Result<ChunkLease, SerError> {
        // Check if an error occurred in the BufMut implementation while serialising:
        if let Some(error) = self.ser_error.take() {
            return Err(error);
        };

        // Check if we've begun a chain
        if let Some(mut chain_head) = self.chain_head.take() {
            // Check if there's anything in the active buffer to append to the chain
            if let Some(tail) = self.encode_buffer.get_chunk_lease() {
                chain_head.append_to_chain(tail);
            }
            // return the chain_head
            Ok(chain_head)
        } else {
            // No chain just return what's written into the active buffers
            self.encode_buffer
                .get_chunk_lease()
                .ok_or_else(|| SerError::InvalidData("No data written".to_string()))
        }
    }

    /// Appends what has been written into the active buffer to the Chain and swaps the Buffer
    fn chain_and_swap(&mut self) -> Result<(), SerError> {
        // Get the written data as a chunk_lease, chain it, and swap the buffer
        if let Some(chunk_lease) = self.encode_buffer.get_chunk_lease() {
            if let Some(chain_head) = &mut self.chain_head {
                // Append to the end of the chain
                chain_head.append_to_chain(chunk_lease);
            } else {
                // Make it the chain head
                self.chain_head = Some(chunk_lease);
            }
        }
        // Swap the buffer
        self.encode_buffer.swap_buffer()
    }
}

/// Make sure the read pointer is caught up to the write pointer for the next encoding.
impl<'a> Drop for BufferEncoder<'a> {
    fn drop(&mut self) {
        self.encode_buffer.read_offset = self.encode_buffer.write_offset;
    }
}

unsafe impl<'a> BufMut for BufferEncoder<'a> {
    fn remaining_mut(&self) -> usize {
        self.encode_buffer.writeable_len()
    }

    unsafe fn advance_mut(&mut self, cnt: usize) {
        if self.ser_error.is_some() {
            return;
        };

        let remaining = self.encode_buffer.writeable_len();
        if remaining < cnt {
            // We need to advance partially, swap, and then advance the rest!
            self.encode_buffer.write_offset += remaining;
            let _ = self.chain_and_swap().map_err(|err| {
                self.ser_error = Some(err);
            });
            self.encode_buffer.write_offset = cnt - remaining;
        } else {
            self.encode_buffer.write_offset += cnt;
        }
    }

    fn bytes_mut(&mut self) -> &mut UninitSlice {
        unsafe {
            let ptr = (&mut *self.encode_buffer.buffer.chunk).as_mut_ptr();
            let offset_ptr = ptr.add(self.encode_buffer.write_offset);
            UninitSlice::from_raw_parts_mut(
                offset_ptr,
                self.encode_buffer.buffer.len() - self.encode_buffer.write_offset,
            )
        }
    }

    fn put<T: super::Buf>(&mut self, mut src: T)
    where
        Self: Sized,
    {
        while src.remaining() > 0 {
            if self.ser_error.is_some() {
                return;
            };
            let len = src.bytes().len();
            self.put_slice(src.bytes());
            src.advance(len);
        }
    }

    /// Override default impl to allow for large slices to be put in at a time
    fn put_slice(&mut self, src: &[u8]) {
        let mut off = 0;

        while off < src.len() {
            if self.ser_error.is_some() {
                return;
            };
            unsafe {
                let dst = self.bytes_mut();
                let cnt = cmp::min(dst.len(), src.len() - off);

                ptr::copy_nonoverlapping(src.as_ptr().add(off), dst.as_mut_ptr() as *mut u8, cnt);

                off += cnt;
                self.advance_mut(cnt);
            }
            // Check if we need to swap
            if self.remaining_mut() < (src.len() - off) {
                // The error will be returned later when get_chunk_lease is called.
                let _ = self.chain_and_swap().map_err(|err| {
                    self.ser_error = Some(err);
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use hocon::HoconLoader;

    // This test instantiates an EncodeBuffer and writes the same string into it enough times that
    // the EncodeBuffer should overload multiple times and will have to succeed in reusing >=1 Chunk
    #[test]
    fn encode_buffer_overload_reuse_default_config() {
        let buffer_config = BufferConfig::default();
        let data_len = buffer_config.chunk_size - buffer_config.encode_buf_min_free_space - 8;
        let encode_buffer = encode_chain_overload_reuse(&buffer_config, data_len);
        // Check the buffer pool sizes
        assert_eq!(
            encode_buffer.buffer_pool.get_pool_sizes(),
            (
                buffer_config.initial_chunk_count, // No additional has been allocated
                buffer_config.initial_chunk_count - 1  // Currently in the pool
            )
        );
    }

    // This test instantiates an EncodeBuffer and writes the same string into it enough times that
    // the EncodeBuffer should overload multiple times and will have to succeed in reusing >=1 Chunk
    #[test]
    fn encode_buffer_overload_reuse_default_config_big_data() {
        let buffer_config = BufferConfig::default();
        // Data length will span more chunks than the initial chunk_count
        let data_len = buffer_config.chunk_size * buffer_config.initial_chunk_count * 2 + 8;
        let encode_buffer = encode_chain_overload_reuse(&buffer_config, data_len);
        // Check the buffer pool sizes
        assert_eq!(
            encode_buffer.buffer_pool.get_pool_sizes(),
            (
                buffer_config.initial_chunk_count * 2 + 1, // One additional chunk has been allocated
                buffer_config.initial_chunk_count * 2      // Currently in the pool
            )
        );
    }

    // As above, except we create a much larger BufferPool with larger Chunks manually configured
    #[test]
    fn encode_buffer_overload_reuse_manually_configured_large_buffers() {
        let mut buffer_config = BufferConfig::default();
        buffer_config.chunk_size(5000000); // 5 MB chunk_size
        buffer_config.initial_chunk_count(5); // 50 MB pool init value
        buffer_config.max_chunk_count(10); // 100 MB pool max size
        buffer_config.encode_buf_min_free_space(256); // 256 B min_remaining
        let data_len = buffer_config.chunk_size - buffer_config.encode_buf_min_free_space - 8;
        let encode_buffer = encode_chain_overload_reuse(&buffer_config, data_len);
        assert_eq!(encode_buffer.buffer.len(), 5000000);
        // Check the buffer pool sizes
        assert_eq!(
            encode_buffer.buffer_pool.get_pool_sizes(),
            (
                5,     // No additional has been allocated
                5 - 1  // Currently in the pool
            )
        );
    }

    // As above, except we create small buffers from a Hocon config.
    #[test]
    fn encode_buffer_overload_reuse_hocon_configured_small_buffers() {
        let hocon = HoconLoader::new()
            .load_str(
                r#"{
            buffer_config {
                chunk_size: 128,
                initial_chunk_count: 2,
                max_pool_count: 2,
                encode_min_remaining: 2,
                }
            }"#,
            )
            .unwrap()
            .hocon();
        let buffer_config = BufferConfig::from_config(&hocon.unwrap());
        // Ensure we successfully parsed the Config
        assert_eq!(buffer_config.encode_buf_min_free_space, 2usize);
        let data_len = buffer_config.chunk_size - buffer_config.encode_buf_min_free_space - 8;
        let encode_buffer = encode_chain_overload_reuse(&buffer_config, data_len);
        assert_eq!(encode_buffer.buffer.len(), 128);
        // Check the buffer pool sizes
        assert_eq!(
            encode_buffer.buffer_pool.get_pool_sizes(),
            (
                2,     // No additional has been allocated
                2 - 1  // Currently in the pool
            )
        );
    }

    // Creates an EncodeBuffer from the given config and uses the data_len to insert a generated-
    // string multiple times such that the Buffer is exhausted.
    fn encode_chain_overload_reuse(buffer_config: &BufferConfig, data_len: usize) -> EncodeBuffer {
        // Instantiate an encode buffer with the given buffer_config.
        let mut encode_buffer = EncodeBuffer::with_config(&buffer_config, &None);
        // Create a string that's bigger than the ENCODEBUFFER_MIN_REMAINING
        // This should produce at least one Chained ChunkLease during our testing
        let mut test_string = "".to_string();
        for i in 0..=data_len {
            // Make sure the string isn't just the same char over and over again,
            // Means we test for some more correctness in buffers
            test_string.push((i.to_string()).chars().next().unwrap());
        }
        let string_bytes = Bytes::copy_from_slice(test_string.as_bytes());
        // We will count how many times chaining happens to ensure that it does happen!
        let mut chain_cnt = 0;
        let mut chunk_lease_cnt = 0;
        // Make sure we churn through all the initial buffers.
        for _ in 0..=(buffer_config.initial_chunk_count + 1) {
            // Make sure we fill up a chunk per iteration:
            for _ in 0..(buffer_config.chunk_size / data_len) + 1 {
                chunk_lease_cnt += 1; // counting chunk
                                      // Get a BufferEncoder interface
                let buffer_encoder = &mut EncodeBuffer::get_buffer_encoder(&mut encode_buffer)
                    .expect("Should not run out of buffers in test case");
                // Insert the test_string
                buffer_encoder.put_slice(test_string.clone().as_bytes());
                // Retrieve the inserted data as a ChunkLease
                let chunk_lease = &mut buffer_encoder.get_chunk_lease().unwrap();
                // capacity() should be equal to the test-string, first assert lengths.
                assert_eq!(
                    chunk_lease.capacity(),
                    test_string.as_bytes().len(),
                    "Incorrect Lengths of ChunkLease created, ChunkLease number = {}",
                    chunk_lease_cnt
                );
                // Bytes only returns the chain-head, remaining is the full length of the chain
                if chunk_lease.bytes().len() < chunk_lease.remaining() {
                    chain_cnt += 1;
                }
                // to_bytes() makes the chain readable in full to a single continuous slice.
                assert_eq!(
                    chunk_lease.copy_to_bytes(chunk_lease.remaining()),
                    string_bytes,
                    "Incorrect Data in ChunkLease, ChunkLease number = {}",
                    chunk_lease_cnt
                );
            }
        }
        // This assertion is dependent on the buffer_config/data_len producing a chaining.
        assert_ne!(chain_cnt, 0, "No chaining occurred");
        encode_buffer
    }

    #[test]
    fn encode_buffer_no_buffer_error() {
        // Instantiate an encode buffer with default values
        let mut buffer_config = BufferConfig::default();
        buffer_config.max_chunk_count(2);
        buffer_config.initial_chunk_count(2); // Only 2 chunks
        let mut encode_buffer = EncodeBuffer::with_config(&buffer_config, &None);
        let buffer_encoder = &mut EncodeBuffer::get_buffer_encoder(&mut encode_buffer)
            .expect("Should not run out of buffers in test case");
        use std::string::ToString;
        let mut test_string = "".to_string();
        for i in 0..=buffer_config.chunk_size * 3 {
            // Enough to fill 3
            test_string.push((i.to_string()).chars().next().unwrap());
        }
        buffer_encoder.put(test_string.as_bytes());
        buffer_encoder
            .get_chunk_lease()
            .expect_err("should return error");
    }

    #[test]
    fn aligned_after_drop() {
        // Instantiate an encode buffer with default values
        let buffer_config = BufferConfig::default();
        let mut encode_buffer = EncodeBuffer::with_config(&buffer_config, &None);
        {
            let buffer_encoder = &mut EncodeBuffer::get_buffer_encoder(&mut encode_buffer)
                .expect("Should not run out of buffers in test case");
            buffer_encoder.put_i32(16);
        }
        // Check the read and write pointers are aligned despite no read call
        assert_eq!(encode_buffer.write_offset, encode_buffer.read_offset);
        assert_eq!(encode_buffer.write_offset, 4);
    }

    #[test]
    fn encode_buffer_get_buffer_encoder_eager_swap() {
        // Instantiate an encode buffer with default values
        let mut buffer_config = BufferConfig::default();
        buffer_config.chunk_size(128);
        buffer_config.encode_buf_min_free_space(64);
        let mut encode_buffer = EncodeBuffer::with_config(&buffer_config, &None);
        {
            let buffer_encoder = &mut EncodeBuffer::get_buffer_encoder(&mut encode_buffer)
                .expect("Should not run out of buffers in test case");
            for i in 0..65 {
                buffer_encoder.put_u8(i);
            }
        }
        assert_eq!(encode_buffer.write_offset, 65);
        {
            // Should have eagerly swapped the buffer!
            let buffer_encoder = &mut EncodeBuffer::get_buffer_encoder(&mut encode_buffer)
                .expect("Should not run out of buffers in test case");
            assert_eq!(buffer_encoder.encode_buffer.write_offset, 0);
        }
    }
}

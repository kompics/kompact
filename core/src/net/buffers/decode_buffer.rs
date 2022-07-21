use super::*;
use std::cmp::Ordering;

/// Used to allow extraction of data frames in between inserting data
/// And can replace the underlying BufferChunk with new ones
pub struct DecodeBuffer {
    buffer: BufferChunk,
    write_offset: usize,
    read_offset: usize,
    next_frame_head: Option<FrameHead>,
    // Used when data spans across multiple chunks
    chain_head: Option<ChunkLease>,
    buffer_config: BufferConfig,
}

impl std::fmt::Debug for DecodeBuffer {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecodeBuffer")
            .field("read_offset", &self.read_offset)
            .field("write_offset", &self.write_offset)
            .field("readable_len", &self.readable_len())
            .field("writeable_len", &self.writeable_len())
            .field("BufferConfig", &self.buffer_config)
            .finish()
    }
}

impl DecodeBuffer {
    /// Creates a new DecodeBuffer from th given BufferChunk.
    pub fn new(buffer: BufferChunk, buffer_config: &BufferConfig) -> Self {
        DecodeBuffer {
            buffer,
            // Offsets only track the current buffer, not the chain_head.
            write_offset: 0,
            read_offset: 0,
            next_frame_head: None,
            chain_head: None,
            buffer_config: buffer_config.clone(),
        }
    }

    /// Returns the length of the read-portion of the buffer AND the chain_head
    pub fn readable_len(&self) -> usize {
        let mut len = 0;
        if let Some(chain_head) = &self.chain_head {
            len += chain_head.remaining()
        }
        len += self.write_offset - self.read_offset;
        len
    }

    /// Returns the length of the write-portion of the buffer
    pub fn writeable_len(&self) -> usize {
        self.buffer.len() - self.write_offset
    }

    /// Advances the write pointer by num_bytes
    pub fn advance_writeable(&mut self, num_bytes: usize) -> () {
        self.write_offset += num_bytes;
    }

    /// Returns an Write compatible slice of the writeable end of the buffer
    /// If something is written to the slice advance writeable must be called after
    pub fn get_writeable(&mut self) -> Option<&mut [u8]> {
        // If the readable portion of the buffer would have less than `encode_buf_min_free_space`
        // Or if we would return less than 8 readable bytes we don't allow further writing into
        // the current buffer, caller must swap.
        if self.is_writeable() {
            unsafe { Some(self.buffer.get_slice(self.write_offset, self.buffer.len())) }
        } else {
            None
        }
    }

    /// True if there is sufficient amount of writeable bytes
    pub(crate) fn is_writeable(&self) -> bool {
        self.writeable_len() > 8
    }

    /// Returns true if there is data to be decoded, else false
    pub(crate) fn has_frame(&mut self) -> io::Result<bool> {
        self.decode_frame_head()?;
        if let Some(head) = &self.next_frame_head {
            if self.readable_len() >= head.content_length() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Swaps the underlying buffer in place with other
    /// Afterwards `other` owns the used up bytes and is locked.
    ///
    /// `get_frame()` should be called repeatedly until it returns `FramingError::NoData`
    /// before calling this method.
    pub fn swap_buffer(&mut self, other: &mut BufferChunk) -> () {
        assert!(other.free(), "Swapping with locked buffer");
        let overflow = self.get_overflow();
        self.buffer.swap_buffer(other);
        self.read_offset = 0;
        self.write_offset = 0;
        if let Some(mut overflow_chunk) = overflow {
            // TODO: change the config parameter to a separate value?
            let overflow_len = overflow_chunk.remaining();
            if overflow_len < self.writeable_len()
                && self.writeable_len() - overflow_len
                    > self.buffer_config.encode_buf_min_free_space
            {
                // Just copy the overflow_chunk bytes, no need to chain.
                // the overflow must not exceed the new buffers capacity
                unsafe {
                    overflow_chunk.copy_to_slice(self.buffer.get_slice(0, overflow_len));
                }
                self.write_offset = overflow_len;
            } else {
                // Start a chain/Append to the chain
                if let Some(chain_head) = &mut self.chain_head {
                    chain_head.append_to_chain(overflow_chunk);
                } else {
                    self.chain_head = Some(overflow_chunk);
                }
            }
        }
        // other has been swapped in-place, can be returned to the pool
        other.lock();
    }

    /// Extracts a ChunkLease from the readable portion of the buffer (including the chain)
    fn read_chunk_lease(&mut self, length: usize) -> ChunkLease {
        if let Some(mut chain_head) = self.chain_head.take() {
            match chain_head.remaining().cmp(&length) {
                Ordering::Greater => {
                    // Need to split the chain_head...
                    let tail = chain_head.split_at(length);
                    // reinsert the remainder of the chain
                    self.chain_head = Some(tail);
                    // return the head of the chain
                    chain_head
                }
                Ordering::Equal => {
                    // The chain_head was all we needed
                    chain_head
                }
                Ordering::Less => {
                    // Need to append to the chain_head from the current buffer
                    let tail_length = length - chain_head.remaining();
                    let tail = self
                        .buffer
                        .get_lease(self.read_offset, self.read_offset + tail_length);
                    self.read_offset += tail_length;
                    chain_head.append_to_chain(tail);
                    chain_head
                }
            }
        } else {
            // No chain
            let lease = self
                .buffer
                .get_lease(self.read_offset, self.read_offset + length);
            self.read_offset += length;
            lease
        }
    }

    /// Tries to decode one frame from the readable part of the buffer
    pub fn get_frame(&mut self) -> Result<Frame, FramingError> {
        self.decode_frame_head()?;
        if let Some(head) = &self.next_frame_head {
            if self.readable_len() >= head.content_length() {
                let head = self.next_frame_head.take().unwrap();
                return match head.frame_type() {
                    // Frames with empty bodies should be handled in frame-head decoding below.
                    FrameType::Data => {
                        Data::decode_from(self.read_chunk_lease(head.content_length()))
                            .map_err(|_| FramingError::InvalidFrame)
                    }
                    FrameType::Hello => {
                        Hello::decode_from(self.read_chunk_lease(head.content_length()))
                            .map_err(|_| FramingError::InvalidFrame)
                    }
                    FrameType::Start => {
                        Start::decode_from(self.read_chunk_lease(head.content_length()))
                            .map_err(|_| FramingError::InvalidFrame)
                    }
                    // Frames without content match here for expediency, Decoder doesn't allow 0 length.
                    FrameType::Bye => Ok(Frame::Bye()),
                    FrameType::Ack => Ok(Frame::Ack()),
                    _ => Err(FramingError::UnsupportedFrameType),
                };
            }
        }
        Err(FramingError::NoData)
    }

    fn decode_frame_head(&mut self) -> Result<(), FramingError> {
        if self.next_frame_head.is_none() && self.readable_len() >= FRAME_HEAD_LEN as usize {
            let mut chunk_lease = self.read_chunk_lease(FRAME_HEAD_LEN as usize);
            let head = FrameHead::decode_from(&mut chunk_lease)?;
            self.next_frame_head = Some(head);
        }
        Ok(())
    }

    /// Extracts the readable portion (if any) from the active buffer as a ChunkLease
    fn get_overflow(&mut self) -> Option<ChunkLease> {
        let length = self.write_offset - self.read_offset;
        if length > 0 {
            let lease = self.buffer.get_lease(self.read_offset, self.write_offset);
            self.read_offset += length;
            return Some(lease);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::{Bytes, BytesMut};

    fn test_frame_with_reference_bytes(len: usize) -> (Vec<u8>, Bytes) {
        let mut head = FrameHead::new(FrameType::Data, (len - 9) as usize);
        let mut frame_bytes = BytesMut::with_capacity(len);
        head.encode_into(&mut frame_bytes);
        for i in 0..len - 9 {
            frame_bytes.put_u8(i as u8);
        }
        let mut reference_bytes = BytesMut::with_capacity((len - 9) as usize);
        for i in 0..len - 9 {
            reference_bytes.put_u8(i as u8);
        }
        (
            frame_bytes.to_vec(),
            reference_bytes.copy_to_bytes(reference_bytes.remaining()),
        )
    }

    /// Creates a DecodeBuffer and a BufferPool, writes multiple Frames into the DecodeBuffer
    /// And swaps the filled Buffers TWICE before we call decode multiple times.
    /// Chunk_size is 128, we write frames of lengths: 64 bytes, 64+128+64 bytes, 64 bytes.
    #[test]
    fn decode_buffer_multi_chain_overflow() {
        let mut cfg = BufferConfig::default();
        cfg.chunk_size(128);
        let mut pool = BufferPool::with_config(&cfg, &None);
        let chunk_1 = pool.get_buffer().unwrap();
        let mut chunk_2 = pool.get_buffer().unwrap();
        let mut chunk_3 = pool.get_buffer().unwrap();
        let mut chunk_4 = pool.get_buffer().unwrap();
        let mut chunk_5 = pool.get_buffer().unwrap();
        let mut decode_buffer = DecodeBuffer::new(chunk_1, &cfg);

        // Frame 1 = 9 Byte FrameHead and 55 Byte content
        let (frame1, reference_bytes_1) = test_frame_with_reference_bytes(64);
        decode_buffer
            .get_writeable()
            .unwrap()
            .put_slice(frame1.as_slice());
        decode_buffer.advance_writeable(64);
        // Frame 2 = 9 Byte FrameHead and 247 Byte content
        let (frame2, reference_bytes_2) = test_frame_with_reference_bytes(256);
        let (frame2a, frame2b) = frame2.split_at(64); // 64 + 192
        let (frame2c, frame2d) = frame2b.split_at(128); // 128 + 64
        decode_buffer.get_writeable().unwrap().put_slice(frame2a);
        decode_buffer.advance_writeable(64);
        decode_buffer.swap_buffer(&mut chunk_2);
        decode_buffer.get_writeable().unwrap().put_slice(frame2c);
        decode_buffer.advance_writeable(128);
        decode_buffer.swap_buffer(&mut chunk_3);
        decode_buffer.get_writeable().unwrap().put_slice(frame2d);
        decode_buffer.advance_writeable(64);

        // Frame 3 = 9 Byte FrameHead and 55 Byte content
        let (frame3, reference_bytes_3) = test_frame_with_reference_bytes(64);
        decode_buffer
            .get_writeable()
            .unwrap()
            .put_slice(frame3.as_slice());
        decode_buffer.advance_writeable(64);
        decode_buffer.swap_buffer(&mut chunk_4);

        // Frame 4 = 9 Byte FrameHead and 119 Byte content
        let (frame4, reference_bytes_4) = test_frame_with_reference_bytes(128);
        decode_buffer
            .get_writeable()
            .unwrap()
            .put_slice(frame4.as_slice());
        decode_buffer.advance_writeable(128);
        decode_buffer.swap_buffer(&mut chunk_5);

        // 4 Chunks have been filled and should be decoded entirely from the chain
        let decoded_frame1 = decode_buffer.get_frame().unwrap();
        let decoded_frame2 = decode_buffer.get_frame().unwrap();
        let decoded_frame3 = decode_buffer.get_frame().unwrap();
        let decoded_frame4 = decode_buffer.get_frame().unwrap();

        // Finally assert that the decoded chunk is equal to corresponding reference bytes
        match decoded_frame1 {
            Frame::Data(decoded_data_1) => {
                let len = decoded_data_1.encoded_len();
                assert_eq!(
                    decoded_data_1.payload().copy_to_bytes(len),
                    reference_bytes_1
                );
            }
            _ => {
                panic!("Improper framing in test case");
            }
        }
        match decoded_frame2 {
            Frame::Data(decoded_data_2) => {
                let len = decoded_data_2.encoded_len();
                assert_eq!(
                    decoded_data_2.payload().copy_to_bytes(len),
                    reference_bytes_2
                );
            }
            _ => {
                panic!("Improper framing in test case");
            }
        }
        match decoded_frame3 {
            Frame::Data(decoded_data_3) => {
                let len = decoded_data_3.encoded_len();
                assert_eq!(
                    decoded_data_3.payload().copy_to_bytes(len),
                    reference_bytes_3
                );
            }
            _ => {
                panic!("Improper framing in test case");
            }
        }
        match decoded_frame4 {
            Frame::Data(decoded_data_4) => {
                let len = decoded_data_4.encoded_len();
                assert_eq!(
                    decoded_data_4.payload().copy_to_bytes(len),
                    reference_bytes_4
                );
            }
            _ => {
                panic!("Improper framing in test case");
            }
        }
    }
}

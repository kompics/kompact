use crate::prelude::Buf;
use std::sync::Arc;

/// A `ChunkRef` is created from a `ChunkLease`, or from a `ChunkRef` and a `ChunkLease`.
/// It is immutable and may be cloned and shared and chained with many other `ChunkRefs`.
#[derive(Debug, Clone)]
pub struct ChunkRef {
    content: &'static [u8],
    read_pointer: usize,
    chain_head_len: usize,
    lock: Arc<u8>,
    chain: Option<Box<ChunkRef>>,
    /// The length of the chain from self (i.e. independent of parent(s))
    chain_len: usize,
}

impl ChunkRef {
    pub(crate) fn new(
        content: &'static [u8],
        read_pointer: usize,
        chain_head_len: usize,
        lock: Arc<u8>,
        chain: Option<Box<ChunkRef>>,
        chain_len: usize,
    ) -> Self {
        Self {
            content,
            read_pointer,
            chain_head_len,
            lock,
            chain,
            chain_len,
        }
    }

    /// The full length of the underlying bytes without read-pointers
    /// Use [remaining()](ChunkRef::remaining) for the remaining readable length.
    pub fn capacity(&self) -> usize {
        self.chain_len
    }

    /// Appends `new_tail` to the end of the `ChunkRef` chain
    /// Use Public ChunkLease methods into_chunk_ref_as_tail or into_chunk_ref_with_tail
    pub(super) fn append_to_chain(&mut self, new_tail: ChunkRef) {
        self.chain_len += new_tail.chain_len;
        if let Some(tail) = &mut self.chain {
            // recursion
            tail.append_to_chain(new_tail)
        } else {
            // recursion complete
            self.chain = Some(Box::new(new_tail))
        }
    }

    // Recursive method for the bytes() impl
    fn get_bytes_at(&self, pos: usize) -> &[u8] {
        if pos >= self.chain_head_len {
            if let Some(chain) = &self.chain {
                chain.get_bytes_at(pos - self.chain_head_len)
            } else {
                panic!("Critical Bug in ChunkRef, bad chain");
            }
        } else {
            let slice: &[u8] = &*self.content;
            &slice[pos..]
        }
    }
}

impl Buf for ChunkRef {
    fn remaining(&self) -> usize {
        self.chain_len - self.read_pointer
    }

    // Returns a slice starting from the read-pointer up to (possibly) the
    fn bytes(&self) -> &[u8] {
        self.get_bytes_at(self.read_pointer)
    }

    fn advance(&mut self, cnt: usize) {
        self.read_pointer += cnt;
    }
}

unsafe impl Send for ChunkRef {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::buffers::{BufferConfig, EncodeBuffer};
    use bytes::{BufMut, Bytes};

    // Use different data sizes and buffer sizes to test different kinds of chains/splits.
    // We want to test Chaining, Creation functions from chunk_lease.rs and reading/using Buf impl
    // Variants:
    //     ChunkRef without chain -> 1 ChunkLease needed with reference Bytes
    //     ChunkRef with chain from Chained ChunkLease -> 2 ChunkLease needed with reference Bytes
    //     A Chained ChunkRef prepended to Chained ChunkLease -> 4 ChunkLeases
    //     A Chained ChunkRef appended to Chained ChunkLease -> 4 ChunkLeases
    fn generate_bytes(data_len: usize, count: usize) -> Vec<Bytes> {
        let mut vec = Vec::with_capacity(count);
        for i in 0..count {
            let mut test_string = "".to_string();
            for j in 0..data_len {
                test_string.push(((j + i * data_len).to_string()).chars().next().unwrap());
            }
            let len = test_string.as_bytes().len();
            vec.push(test_string.as_bytes().copy_to_bytes(len));
        }
        vec
    }

    fn get_testing_encode_buffer() -> EncodeBuffer {
        let mut cfg = BufferConfig::default();
        cfg.chunk_size(128);
        cfg.initial_chunk_count(2);
        // let pool = BufferPool::with_config(&cfg, &None);
        EncodeBuffer::with_config(&cfg, &None)
    }

    #[test]
    fn chunk_ref_unchained() {
        let byte_vec = generate_bytes(64, 1);
        let mut encode_buffer = get_testing_encode_buffer();

        {
            let mut chunk_ref = {
                // Scoping the buffer encoder and chunk_lease to count locks properly...
                let mut buffer_encoder = encode_buffer
                    .get_buffer_encoder()
                    .expect("Should not run out of buffers in test case");
                buffer_encoder.put(byte_vec[0].clone());
                buffer_encoder.get_chunk_lease().unwrap().into_chunk_ref()
            };

            // Swap the underlying buffer and make sure the returned buffer can't be unlocked
            encode_buffer
                .swap_buffer()
                .expect("Should not run out of buffers in test case");
            assert_eq!(encode_buffer.buffer_pool.count_locked_chunks(), 1);

            // Assert that the bytes are correct
            assert_eq!(chunk_ref.copy_to_bytes(chunk_ref.remaining()), byte_vec[0]);
        }
        // Should be released as they are now out of scope
        assert_eq!(encode_buffer.buffer_pool.count_locked_chunks(), 0);
    }

    #[test]
    fn chunk_ref_from_chained() {
        let byte_vec = generate_bytes(256, 1);
        let mut encode_buffer = get_testing_encode_buffer();

        {
            let mut chunk_ref = {
                // Scoping the buffer encoder and chunk_lease to count locks properly...
                let mut buffer_encoder = encode_buffer
                    .get_buffer_encoder()
                    .expect("Should not run out of buffers in test case");
                buffer_encoder.put(byte_vec[0].clone());
                buffer_encoder.get_chunk_lease().unwrap().into_chunk_ref()
            };
            assert!(chunk_ref.chain.is_some());
            // Swap the underlying buffer and make sure the returned buffer can't be unlocked
            encode_buffer
                .swap_buffer()
                .expect("Should not run out of buffers in test case");
            assert_eq!(encode_buffer.buffer_pool.count_locked_chunks(), 2);

            // Assert that the bytes are correct
            assert_eq!(chunk_ref.copy_to_bytes(chunk_ref.remaining()), byte_vec[0]);
        }
        // Should be released as they are now out of scope
        assert_eq!(encode_buffer.buffer_pool.count_locked_chunks(), 0);
    }

    #[test]
    fn chunk_ref_chained_with_head() {
        let byte_vec = generate_bytes(256, 2);
        let mut encode_buffer = get_testing_encode_buffer();

        {
            let mut chunk_ref = {
                // Scoping the buffer encoder and chunk_lease to count locks properly...
                let mut buffer_encoder = encode_buffer
                    .get_buffer_encoder()
                    .expect("Should not run out of buffers in test case");
                // Create the head
                buffer_encoder.put(byte_vec[0].clone());
                let head = buffer_encoder.get_chunk_lease().unwrap().into_chunk_ref();
                // Create the tail and prepend the head to it
                buffer_encoder.put(byte_vec[1].clone());
                buffer_encoder
                    .get_chunk_lease()
                    .unwrap()
                    .into_chunk_ref_with_head(head)
            };
            assert!(chunk_ref.chain.is_some());
            // Swap the underlying buffer and make sure the returned buffer can't be unlocked
            encode_buffer
                .swap_buffer()
                .expect("Should not run out of buffers in test case");
            assert_eq!(encode_buffer.buffer_pool.count_locked_chunks(), 4);

            // Assert that the bytes are correct
            let comp_vec = generate_bytes(512, 1);

            assert_eq!(chunk_ref.copy_to_bytes(chunk_ref.remaining()), comp_vec[0]);
        }
        // Should be released as they are now out of scope
        assert_eq!(encode_buffer.buffer_pool.count_locked_chunks(), 0);
    }

    #[test]
    fn chunk_ref_append_chained_to_chain() {
        let byte_vec = generate_bytes(256, 2);
        let mut encode_buffer = get_testing_encode_buffer();

        {
            let mut chunk_ref = {
                // Scoping the buffer encoder and chunk_lease to count locks properly...
                let mut buffer_encoder = encode_buffer
                    .get_buffer_encoder()
                    .expect("Should not run out of buffers in test case");
                // Create the head
                buffer_encoder.put(byte_vec[1].clone());
                let tail = buffer_encoder.get_chunk_lease().unwrap().into_chunk_ref();
                // Create the tail and prepend the head to it
                buffer_encoder.put(byte_vec[0].clone());
                buffer_encoder
                    .get_chunk_lease()
                    .unwrap()
                    .into_chunk_ref_with_tail(tail)
            };
            assert!(chunk_ref.chain.is_some());
            // Swap the underlying buffer and make sure the returned buffer can't be unlocked
            encode_buffer
                .swap_buffer()
                .expect("Should not run out of buffers in test case");
            assert_eq!(encode_buffer.buffer_pool.count_locked_chunks(), 4);

            // Assert that the bytes are correct
            let comp_vec = generate_bytes(512, 1);

            assert_eq!(chunk_ref.copy_to_bytes(chunk_ref.remaining()), comp_vec[0]);
        }
        // Should be released as they are now out of scope
        assert_eq!(encode_buffer.buffer_pool.count_locked_chunks(), 0);
    }
}

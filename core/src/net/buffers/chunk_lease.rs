use super::*;
use bytes::{buf::UninitSlice, Bytes};
use std::cmp::Ordering;

/// A ChunkLease is a smart-pointer to a byte-slice, implementing [Buf](bytes::Buf) and
/// [BufMut](bytes::BufMut) interfaces. They are created with one or many distinct slices of
/// [BufferChunks](BufferChunk) and holds a lock preventing the data in the byte-slice(s) from being
/// modified until the `ChunkLease` is dropped.
#[derive(Debug)]
pub struct ChunkLease {
    content: &'static mut [u8],
    write_pointer: usize,
    read_pointer: usize,
    chain_head_len: usize,
    lock: Arc<u8>,
    chain: Option<Box<ChunkLease>>,
    /// The length of the chain from self (i.e. independent of parent(s))
    chain_len: usize,
}

impl ChunkLease {
    /// Creates a new `ChunkLease` from a static byte-slice with already written bytes and a `lock`.
    pub fn new(content: &'static mut [u8], lock: Arc<u8>) -> ChunkLease {
        let capacity = content.len();
        let write_pointer = content.len();
        ChunkLease {
            content,
            write_pointer,
            chain_head_len: capacity,
            lock,
            read_pointer: 0,
            chain: None,
            chain_len: capacity,
        }
    }

    /// The full length of the underlying bytes without read/write-pointers
    /// Use `remaining()`/`remaining_mut()` for readable/writable lengths
    pub fn capacity(&self) -> usize {
        self.chain_len
    }

    /// This inserts a FrameHead at the head of the Chunklease, the ChunkLease should be padded manually
    /// before this method is invoked, i.e. it does not create space for the head on its own.
    ///
    /// Proper framing thus requires 1. pad(), 2 serialise into DecodeBuffer, 3. get_chunk_lease, 4. insert_head
    pub(crate) fn insert_head(&mut self, mut head: FrameHead) {
        // Store the write pointer
        let written = self.write_pointer;
        // Move write-pointer to the front of the buffer:
        self.write_pointer = 0;
        // Encode the into self
        head.encode_into(self);
        // Restore write-pointer
        self.write_pointer = written;
    }

    /// Appends `new_tail` to the end of the `ChunkLease` chain
    pub(crate) fn append_to_chain(&mut self, new_tail: ChunkLease) {
        self.chain_len += new_tail.chain_len;
        if let Some(tail) = &mut self.chain {
            // recursion
            tail.append_to_chain(new_tail);
        } else {
            // recursion complete
            self.chain = Some(Box::new(new_tail))
        }
    }

    /// Splits self into two `ChunkLeases`  such that `self` has the length of `position`
    /// This operation does not account for read and write pointers.
    pub(crate) fn split_at(&mut self, position: usize) -> ChunkLease {
        // Recursion by decrementing position in each recursive step
        assert!(
            position < self.remaining(),
            "Trying to split at bad position"
        );
        self.chain_len = position;
        match position.cmp(&self.chain_head_len) {
            Ordering::Greater => {
                // Do recursion
                if let Some(tail) = &mut self.chain {
                    return tail.split_at(position - self.chain_head_len);
                }
            }
            Ordering::Equal => {
                // Simple split, take the chain out and return it
                if let Some(tail_chain) = self.chain.take() {
                    return *tail_chain;
                }
            }
            Ordering::Less => {
                // Split of the data in self and retain self while returning the "tail" of the split
                unsafe {
                    let content_ptr = (&mut *self.content).as_mut_ptr();
                    let head_bytes = std::slice::from_raw_parts_mut(content_ptr, position);
                    let tail_ptr = content_ptr.add(position);
                    let tail_bytes =
                        std::slice::from_raw_parts_mut(tail_ptr, self.chain_head_len - position);
                    self.content = head_bytes;
                    self.chain_head_len = position;
                    let mut return_lease = ChunkLease::new(tail_bytes, self.lock.clone());
                    if let Some(tail_chain) = self.chain.take() {
                        return_lease.append_to_chain(*tail_chain);
                    }
                    return return_lease;
                }
            }
        }
        // Should not be reachable
        panic!(
            "Trying to split faulty ChunkLease {:#?} at position: {}, should never happen",
            &self, &position
        );
    }

    // Recursive method for the bytes() impl
    fn get_bytes_at(&self, pos: usize) -> &[u8] {
        if pos >= self.chain_head_len {
            if let Some(chain) = &self.chain {
                chain.get_bytes_at(pos - self.chain_head_len)
            } else {
                panic!("Critical Bug in ChunkLease, bad chain");
            }
        } else {
            let slice: &[u8] = &*self.content;
            &slice[pos..]
        }
    }

    // Recursive method for the bytes_mut() impl
    fn get_bytes_mut_at(&mut self, pos: usize) -> &mut UninitSlice {
        if pos >= self.chain_head_len {
            if let Some(chain) = &mut self.chain {
                chain.get_bytes_mut_at(pos - self.chain_head_len)
            } else {
                panic!("Critical Bug in ChunkLease, bad chain");
            }
        } else {
            unsafe {
                let offset_ptr = self.content.as_mut_ptr().add(pos);
                UninitSlice::from_raw_parts_mut(offset_ptr, self.chain_head_len - pos)
            }
        }
    }

    /// Transforms this `ChunkLease` into a [ChunkRef](ChunkRef), an immutable and cloneable smart-pointer
    pub fn into_chunk_ref(self) -> ChunkRef {
        let chain = {
            if let Some(chain) = self.chain {
                Some(Box::new(chain.into_chunk_ref()))
            } else {
                None
            }
        };
        ChunkRef::new(
            self.content,
            self.read_pointer,
            self.chain_head_len,
            self.lock,
            chain,
            self.chain_len,
        )
    }

    /// Creates a chained [ChunkRef](ChunkRef) with `tail` at the end of the new `ChunkRef`.
    pub fn into_chunk_ref_with_tail(self, tail: ChunkRef) -> ChunkRef {
        let mut chunk_ref = self.into_chunk_ref();
        chunk_ref.append_to_chain(tail);
        chunk_ref
    }

    /// Creates a chained [ChunkRef](ChunkRef) with `head` at the front of the new `ChunkRef`.
    pub fn into_chunk_ref_with_head(self, mut head: ChunkRef) -> ChunkRef {
        let chunk_ref = self.into_chunk_ref();
        head.append_to_chain(chunk_ref);
        head
    }

    /// Creates a byte-clone of the contents of the *remaining* bytes within the ChunkLease.
    /// This is a costly operation and should be avoided.
    pub fn create_byte_clone(&self) -> Bytes {
        let mut buf: Vec<u8> = Vec::with_capacity(self.remaining());
        let mut read_pointer = self.read_pointer;
        while read_pointer < self.chain_len {
            let read_bytes = self.get_bytes_at(read_pointer);
            buf.extend_from_slice(read_bytes);
            read_pointer += read_bytes.len();
        }
        Bytes::from(buf)
    }
}

impl Buf for ChunkLease {
    fn remaining(&self) -> usize {
        self.chain_len - self.read_pointer
    }

    fn bytes(&self) -> &[u8] {
        self.get_bytes_at(self.read_pointer)
    }

    fn advance(&mut self, cnt: usize) {
        self.read_pointer += cnt;
    }
}

// BufMut currently only used for injecting a FrameHead at the front.
unsafe impl BufMut for ChunkLease {
    fn remaining_mut(&self) -> usize {
        self.chain_len - self.write_pointer
    }

    unsafe fn advance_mut(&mut self, cnt: usize) {
        self.write_pointer += cnt;
    }

    fn bytes_mut(&mut self) -> &mut UninitSlice {
        self.get_bytes_mut_at(self.write_pointer)
    }
}

unsafe impl Send for ChunkLease {}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    // Use different data sizes and buffer sizes to test different kinds of chains/splits.
    // Creates two test-strings both of length data_len
    fn chain_and_split_test(
        chunk_len: usize,
        data_len: usize,
    ) -> (EncodeBuffer, ChunkLease, ChunkLease) {
        // Create an EncodeBuffer with a small chunk_size
        // Inserts two test-strings into the EncodeBuffer, both of which spans multiple chunk-leases
        // the two strings are extracted as a single continuous chunk-lease.
        // The ChunkLease is then split and the two halves are compared to the original test-strings
        let mut cfg = BufferConfig::default();
        cfg.chunk_size(chunk_len);
        cfg.initial_chunk_count(2);
        // let pool = BufferPool::with_config(&cfg, &None);
        let mut encode_buffer = EncodeBuffer::with_config(&cfg, &None);

        // Create some data
        let mut test_string = "".to_string();
        for i in 0..data_len {
            test_string.push((i.to_string()).chars().next().unwrap());
        }
        let mut test_string2 = "".to_string();
        for i in data_len..data_len * 2 {
            test_string2.push((i.to_string()).chars().next().unwrap());
        }
        // Create a ChunkLease with two identical messages from the data
        let mut both_strings = {
            let mut buffer_encoder = encode_buffer.get_buffer_encoder();
            buffer_encoder.put_slice(test_string.as_bytes());
            buffer_encoder.put_slice(test_string2.as_bytes());
            buffer_encoder.get_chunk_lease().unwrap()
        };

        // Assert the lengths before split
        assert_eq!(both_strings.remaining(), test_string.as_bytes().len() * 2);

        // Split the double down the middle
        let mut second_half = both_strings.split_at(both_strings.remaining() / 2);
        let mut first_half = both_strings; // easier to read assertions

        // Assert lengths after split
        assert_eq!(second_half.remaining(), first_half.remaining());
        assert_eq!(test_string.as_bytes().len(), second_half.remaining());

        let test_bytes = Bytes::copy_from_slice(test_string.as_bytes());
        let test_bytes2 = Bytes::copy_from_slice(test_string2.as_bytes());

        // Assert that create_byte_clone() works
        assert_eq!(test_bytes, first_half.create_byte_clone());
        assert_eq!(test_bytes2, second_half.create_byte_clone());

        // Assert the content is correct
        assert_eq!(test_bytes, first_half.copy_to_bytes(first_half.remaining()));
        assert_eq!(
            test_bytes2,
            second_half.copy_to_bytes(second_half.remaining())
        );

        encode_buffer.swap_buffer(); // ensure that the last chunk is swapped

        (encode_buffer, first_half, second_half)
    }

    // Three test cases for the chain and split:
    // 1st with chains before and after split
    // 2nd without no chains
    // 3rd with chaining before split but not after (split at chain boundary)
    // Assertions on byte-contents, lengths done in test function
    #[test]
    fn chunk_lease_chain_and_split_with_chains() {
        let (mut encode_buffer, first, second) = chain_and_split_test(128, 300);
        // We've asserted the bytes are correct now we check the tests unique properties
        assert!(first.chain.is_some());
        assert!(second.chain.is_some());
        assert_eq!(
            encode_buffer.buffer_pool.count_locked_chunks(),
            600 / 128 + 1
        );
    }

    #[test]
    fn chunk_lease_chain_and_split_without_chains() {
        let (mut encode_buffer, first, second) = chain_and_split_test(128, 64);
        assert!(first.chain.is_none());
        assert!(second.chain.is_none());
        assert_eq!(encode_buffer.buffer_pool.count_locked_chunks(), 1);
    }

    #[test]
    fn chunk_lease_chain_and_split_with_distinct_chunks() {
        let (mut encode_buffer, first, second) = chain_and_split_test(128, 128);
        assert!(first.chain.is_none());
        assert!(second.chain.is_none());
        assert_eq!(encode_buffer.buffer_pool.count_locked_chunks(), 2);
    }
}

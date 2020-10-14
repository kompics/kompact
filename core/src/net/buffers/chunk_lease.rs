use super::*;
use std::cmp::Ordering;

/// A ChunkLease is a Byte-buffer which locks the BufferChunk which the ChunkLease is a lease from.
/// Can be used both as Buf of BufMut depending on what is needed.
#[derive(Debug)]
pub struct ChunkLease {
    content: &'static mut [u8],
    written: usize,
    read: usize,
    chain_head_len: usize,
    lock: Arc<u8>,
    chain: Option<Box<ChunkLease>>,
    /// The length of the chain from self (i.e. independent of parent(s))
    chain_len: usize,
}

impl ChunkLease {
    /// Creates a new ChunkLease from a static byte-slice with already written bytes and a `lock`.
    pub fn new(content: &'static mut [u8], lock: Arc<u8>) -> ChunkLease {
        let capacity = content.len();
        let written = content.len();
        ChunkLease {
            content,
            written,
            chain_head_len: capacity,
            lock,
            read: 0,
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
    /// before this method is invoked, i.e. it does not create space for the head on its own
    /// Proper framing thus requires 1. pad(), 2 serialise into DecodeBuffer, 3. get_chunk_lease, 4. insert_head
    pub(crate) fn insert_head(&mut self, mut head: FrameHead) {
        // Store the write pointer
        let written = self.written;
        // Move write-pointer to the front of the buffer:
        self.written = 0;
        // Encode the into self
        head.encode_into(self);
        // Restore write-pointer
        self.written = written;
    }

    /// Appends `new_tail` to the end of the ChunkLease chain
    pub(crate) fn chain(&mut self, new_tail: ChunkLease) {
        self.chain_len += new_tail.chain_len;
        if let Some(tail) = &mut self.chain {
            // recursion
            tail.chain(new_tail)
        } else {
            // recursion complete
            self.chain = Some(Box::new(new_tail))
        }
    }

    /// Splits self into two ChunkLeases such that the self has the length of `position`
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
                // Split of the data in chain_head and retain self as the "tail"
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
                        return_lease.chain(*tail_chain);
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
    fn get_bytes_mut_at(&mut self, pos: usize) -> &mut [MaybeUninit<u8>] {
        if pos >= self.chain_head_len {
            if let Some(chain) = &mut self.chain {
                chain.get_bytes_mut_at(pos - self.chain_head_len)
            } else {
                panic!("Critical Bug in ChunkLease, bad chain");
            }
        } else {
            unsafe {
                let slice: &mut [u8] = &mut *self.content;
                &mut *(&mut slice[pos..self.chain_head_len] as *mut [u8]
                    as *mut [std::mem::MaybeUninit<u8>])
            }
        }
    }
}

impl Buf for ChunkLease {
    fn remaining(&self) -> usize {
        self.chain_len - self.read
    }

    // Returns a slice starting from the read-pointer up to (possibly) the
    fn bytes(&self) -> &[u8] {
        self.get_bytes_at(self.read)
    }

    fn advance(&mut self, cnt: usize) {
        self.read += cnt;
    }
}

// BufMut currently only used for injecting a FrameHead at the front.
impl BufMut for ChunkLease {
    fn remaining_mut(&self) -> usize {
        self.chain_len - self.written
    }

    unsafe fn advance_mut(&mut self, cnt: usize) {
        self.written += cnt;
    }

    fn bytes_mut(&mut self) -> &mut [MaybeUninit<u8>] {
        self.get_bytes_mut_at(self.written)
    }
}

unsafe impl Send for ChunkLease {}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    /// Three test cases for the chain and split:
    /// 1st with chains before and after split
    /// 2nd without any chains
    /// 3rd with chaining before split but not after
    /// Assertions on byte-contents, lengths, and ChunkLease.chain.is_some()/is_none()
    #[test]
    fn chunk_lease_chain_and_split_with_chains() {
        // Create an EncodeBuffer with a small chunk_size
        // Inserts two test-strings into the EncodeBuffer, both of which spans multiple chunk-leases
        // the two strings are extracted as a single continuous chunk-lease.
        // The ChunkLease is then split and the two halves are compared to the original test-strings
        let mut cfg = BufferConfig::default();
        cfg.chunk_size(128);
        cfg.initial_chunk_count(2);
        // let pool = BufferPool::with_config(&cfg, &None);
        let mut encode_buffer = EncodeBuffer::with_config(&cfg, &None);

        // Create some data
        let mut test_string = "".to_string();
        for i in 0..300 {
            test_string.push((i.to_string()).chars().next().unwrap());
        }
        let mut test_string2 = "".to_string();
        for i in 300..600 {
            test_string2.push((i.to_string()).chars().next().unwrap());
        }
        {
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
            // Assert bytes are intact:
            assert_eq!(test_bytes, first_half.to_bytes());
            assert_eq!(test_bytes2, second_half.to_bytes());

            // Assert that chunks are locked correctly
            encode_buffer.swap_buffer(); // ensure that the last chunk is swapped
            assert_eq!(
                encode_buffer.buffer_pool.count_locked_chunks(),
                (600 / 128) + 1
            );

            // Assert that both have chains (thanks to the chosen lengths)
            assert!(first_half.chain.is_some());
            assert!(second_half.chain.is_some());
        }
        assert_eq!(encode_buffer.buffer_pool.count_locked_chunks(), 0);
    }

    #[test]
    fn chunk_lease_chain_and_split_without_chains() {
        // Create an EncodeBuffer with a small chunk_size
        // Inserts two test-strings into the EncodeBuffer, both of which spans multiple chunk-leases
        // the two strings are extracted as a single continuous chunk-lease.
        // The ChunkLease is then split and the two halves are compared to the original test-strings
        let mut cfg = BufferConfig::default();
        cfg.chunk_size(128);
        cfg.initial_chunk_count(2);
        let mut encode_buffer = EncodeBuffer::with_config(&cfg, &None);

        // Create some data
        let mut test_string = "".to_string();
        let mut test_string2 = "".to_string();
        for i in 0..64 {
            test_string.push((i.to_string()).chars().next().unwrap());
        }
        for i in 64..128 {
            test_string2.push((i.to_string()).chars().next().unwrap());
        }
        {
            // Create a ChunkLease with two identical messages from the data
            let mut both_strings = {
                let mut buffer_encoder = encode_buffer.get_buffer_encoder();
                buffer_encoder.put_slice(test_string.as_bytes());
                buffer_encoder.put_slice(test_string2.as_bytes());
                buffer_encoder.get_chunk_lease().unwrap()
            };

            // Assert the lengths before split
            assert_eq!(both_strings.remaining(), test_string.as_bytes().len() * 2);
            // Assert that it is not a chain
            assert!(both_strings.chain.is_none());

            // Split the double down the middle
            let mut second_half = both_strings.split_at(both_strings.remaining() / 2);
            let mut first_half = both_strings;
            // Assert lengths after split
            assert_eq!(second_half.remaining(), first_half.remaining());
            assert_eq!(test_string.as_bytes().len(), second_half.remaining());

            let test_bytes = Bytes::copy_from_slice(test_string.as_bytes());
            let test_bytes2 = Bytes::copy_from_slice(test_string2.as_bytes());
            // Assert bytes are intact:
            assert_eq!(test_bytes, first_half.to_bytes());
            assert_eq!(test_bytes2, second_half.to_bytes());

            // Assert that chunks are locked correctly
            encode_buffer.swap_buffer(); // ensure that the last chunk is swapped
            assert_eq!(encode_buffer.buffer_pool.count_locked_chunks(), 1);

            // Assert that neither has a chain (thanks to the chosen lengths)
            assert!(first_half.chain.is_none());
            assert!(second_half.chain.is_none());
        }
        assert_eq!(encode_buffer.buffer_pool.count_locked_chunks(), 0);
    }

    #[test]
    fn chunk_lease_chain_and_split_with_distinct_chunks() {
        // Create an EncodeBuffer with a small chunk_size
        // Inserts two test-strings into the EncodeBuffer, both of which spans multiple chunk-leases
        // the two strings are extracted as a single continuous chunk-lease.
        // The ChunkLease is then split and the two halves are compared to the original test-strings
        let mut cfg = BufferConfig::default();
        cfg.chunk_size(128);
        cfg.initial_chunk_count(2);
        let mut encode_buffer = EncodeBuffer::with_config(&cfg, &None);

        // Create some data
        let mut test_string = "".to_string();
        let mut test_string2 = "".to_string();
        for i in 0..128 {
            test_string.push((i.to_string()).chars().next().unwrap());
        }
        for i in 128..256 {
            test_string2.push((i.to_string()).chars().next().unwrap());
        }
        // We scope all the ChunkLeases so we can see if they release the locks
        {
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
            let mut first_half = both_strings;
            // Assert lengths after split
            assert_eq!(second_half.remaining(), first_half.remaining());
            assert_eq!(test_string.as_bytes().len(), second_half.remaining());

            let test_bytes = Bytes::copy_from_slice(test_string.as_bytes());
            let test_bytes2 = Bytes::copy_from_slice(test_string2.as_bytes());
            // Assert bytes are intact:
            assert_eq!(test_bytes, first_half.to_bytes());
            assert_eq!(test_bytes2, second_half.to_bytes());

            // Assert that chunks are locked correctly
            encode_buffer.swap_buffer(); // ensure that the second chunk is swapped
            assert_eq!(encode_buffer.buffer_pool.count_locked_chunks(), 2);
            // Assert that neither has a chain (thanks to the chosen lengths)
            assert!(first_half.chain.is_none());
            assert!(second_half.chain.is_none());
        }
        // ChunkLeases dropped when out of scope, no more locks
        assert_eq!(encode_buffer.buffer_pool.count_locked_chunks(), 0);
    }
}

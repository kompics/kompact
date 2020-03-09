use bytes::{BytesMut, BufMut, Buf, Bytes};
use iovec::IoVec;
use crate::net::frames::*;
use crate::net::frames;
use std::collections::VecDeque;
use std::pin::Pin;
use std::ops::{DerefMut, Deref};
use std::sync::Arc;
use std::borrow::Borrow;
use std::mem::MaybeUninit;
use crate::net::buffer_pool::BufferPool;
use core::{mem, cmp, ptr};

const FRAME_HEAD_LEN: usize = frames::FRAME_HEAD_LEN as usize;
const BUFFER_SIZE: usize = ENCODEBUFFER_MIN_REMAINING * 1000;
// Assume 64 byte cache lines -> 1000 cache lines per chunk
const ENCODEBUFFER_MIN_REMAINING: usize = 64; // Always have at least a full cache line available

/// BufferChunk is a lockable pinned byte-slice
pub struct BufferChunk {
    chunk: Pin<Box<[u8; BUFFER_SIZE]>>,
    ref_count: Arc<u8>,
    locked: bool,
}

impl BufferChunk {
    /// Allocate a new unlocked BufferChunk
    pub fn new() -> Self {
        let mut slice = ([0u8; BUFFER_SIZE]);
        BufferChunk {
            chunk: Pin::new(Box::new(slice)),
            ref_count: Arc::new(0),
            locked: false,
        }
    }

    /// Get the length of the BufferChunk
    pub fn len(&self) -> usize {
        self.chunk.len()
    }

    /// Return a pointer to a subslice of the chunk
    pub unsafe fn get_slice(&mut self, from: usize, to: usize) -> &'static mut [u8] {
        assert!(from < to && to <= self.len() && !self.locked);
        let ptr = self.chunk.as_mut_ptr();
        let offset_ptr = ptr.offset(from as isize);
        std::slice::from_raw_parts_mut(offset_ptr, to - from)
    }

    /// Swaps the pointers of the two buffers such that they effectively switch place
    pub fn swap_buffer(&mut self, other: &mut BufferChunk) -> () {
        std::mem::swap(self.get_chunk_mut(), other.get_chunk_mut());
        std::mem::swap(self.get_lock_mut(), other.get_lock_mut());
    }
    /*
        pub fn get_bytes(&mut self, from: usize, to: usize) -> Bytes {
            let slice = unsafe {
                self.get_slice(from, to)
            };
            let mut bytes_mut = BytesMut::with_capacity(to-from);
            bytes_mut.extend_from_slice(slice);
            return bytes_mut.freeze();
            //return bytes::Bytes::from(slice.to_vec())
        }*/

    /// Returns a ChunkLease pointing to the subslice between `from` and `to`of the BufferChunk
    /// the returned lease is a consumable Buf.
    pub fn get_lease(&mut self, from: usize, to: usize) -> ChunkLease {
        unsafe {
            ChunkLease::new(self.get_slice(from, to), self.get_lock())
        }
    }

    /// Returns a ChunkLease pointing to the subslice between `from` and `to`of the BufferChunk
    /// the returned lease is a writable BufMut.
    pub fn get_free_lease(&mut self, from: usize, to: usize) -> ChunkLease {
        unsafe {
            ChunkLease::new_unused(self.get_slice(from, to), self.get_lock())
        }
    }

    /// Clones the lock, the BufferChunk will be locked until all given locks are deallocated
    pub fn get_lock(&self) -> Arc<u8> {
        self.ref_count.clone()
    }

    /// Returns a mutable pointer to the full underlying byte-slice.
    pub fn get_chunk_mut(&mut self) -> &mut Pin<Box<[u8; BUFFER_SIZE]>> {
        &mut self.chunk
    }

    /// Returns a mutable pointer to the associated lock.
    pub fn get_lock_mut(&mut self) -> &mut Arc<u8> {
        &mut self.ref_count
    }

    /// Returns true if this smart_buffer is available again
    pub fn free(&mut self) -> bool {
        if self.locked {
            if Arc::strong_count(&self.ref_count) < 2 {
                self.locked = false;
                true
            } else {
                false
            }
        } else {
            true
        }
    }

    /// Locks the BufferChunk, it will not be writable nor will any leases be created before it has been unlocked.
    pub fn lock(&mut self) -> () {
        self.locked = true;
    }
}

/// Structure used by components to continuously extract leases from BufferChunks swapped with the internal/private BufferPool
/// Provides a BufMut interface to a private BufferPool, such that data can be serialised into it
/// continuously and ChunkLeases can be extracted for transfer of the serialised data.
/// buffer is automatically swapped with the BufferPool when:
///     1. advance_mut results in less than ENCODEBUFFER_MIN_REMAINING
///     2. put() is called and it won't fit.
///     3. put_slice() is called and it won't fit.
pub struct EncodeBuffer {
    buffer: BufferChunk,
    buffer_pool: BufferPool,
    write_offset: usize,
    read_offset: usize,
}

impl EncodeBuffer {
    /// Creates a new EncodeBuffer, allocates a new BufferPool.
    pub fn new() -> Self {
        let mut buffer_pool = BufferPool::new();
        if let Some(mut buffer) = buffer_pool.get_buffer() {
            EncodeBuffer {
                buffer,
                buffer_pool,
                write_offset: 0,
                read_offset: 0,
            }
        } else {
            panic!("Couldn't initialize EncodeBuffer");
        }
    }

    /// Returns a writable ChunkLease of length `size` if the BufferPool has available Buffers to use.
    pub fn get_buffer(&mut self, size: usize) -> Option<ChunkLease> {
        if self.remaining() > size {
            self.write_offset += size;
            return Some(self.buffer.get_free_lease(self.write_offset - size, self.write_offset));
        } else {
            if let Some(mut new_buffer) = self.buffer_pool.get_buffer() {
                self.swap_buffer();
                return Some(self.buffer.get_free_lease(self.write_offset - size, self.write_offset));
            }
        }
        None
    }
    /// Extracts the bytes between the read-pointer and the write-pointer and advances the read-pointer
    /// Ensures there's a minimum of ENCODEBUFFER_MIN_REMAINING left in the current buffer
    pub fn get_chunk(&mut self) -> Option<ChunkLease> {
        let cnt = self.write_offset - self.read_offset;
        if cnt > 0 {
            self.read_offset += cnt;
            let lease = self.buffer.get_lease(self.write_offset - cnt, self.write_offset);

            if self.remaining() < ENCODEBUFFER_MIN_REMAINING {
                self.swap_buffer();
            }

            return Some(lease);
        }
        None
    }

    /// Returns the remaining length of the current BufferChunk.
    pub fn remaining(&self) -> usize {
        self.buffer.len() - self.write_offset
    }

    /// Swap the current buffer with a fresh one from the private buffer_pool
    /// Copies any unread overflow and sets the write pointer accordingly
    pub fn swap_buffer(&mut self) {
        let overflow = self.get_chunk();
        if let Some(mut new_buffer) = self.buffer_pool.get_buffer() {
            self.buffer.swap_buffer(&mut new_buffer);
            self.read_offset = 0;
            if let Some(chunk) = overflow {
                self.put_slice(chunk.content);
                //self.write_offset = chunk.remaining();
            } else {
                self.write_offset = 0;
            }
            // new_buffer has been swapped in-place, return it to the pool
            self.buffer_pool.return_buffer(new_buffer);
        } else {
            panic!("Trying to swap buffer but no free buffers found in the pool!")
        }
    }

    /// This method advances the write_offset by cnt
    /// Used to reserve space for FrameHeads, enables us to serialize without knowing the size beforehand
    pub(crate) fn pad(&mut self, cnt: usize) {
        self.write_offset += cnt;
    }
}


impl BufMut for EncodeBuffer {
    fn remaining_mut(&self) -> usize {
        self.remaining()
    }

    unsafe fn advance_mut(&mut self, cnt: usize) {
        //self.advance(cnt);
        self.write_offset += cnt;
        if self.remaining() < ENCODEBUFFER_MIN_REMAINING {
            self.swap_buffer();
        }
    }

    fn bytes_mut(&mut self) -> &mut [MaybeUninit<u8>] {
        let ptr = self.buffer.chunk.as_mut_ptr();
        unsafe {
            let offset_ptr = ptr.offset(self.write_offset as isize);
            return mem::transmute(std::slice::from_raw_parts_mut(offset_ptr, self.buffer.chunk.len() - self.write_offset));
        }
        //unsafe { mem::transmute(&mut *&(self.content).offset(self.written as isize)) }
    }

    fn put<T: super::Buf>(&mut self, mut src: T) where Self: Sized {
        assert!(src.remaining() <= BUFFER_SIZE, "src too big for buffering");

        if self.remaining_mut() < src.remaining() {
            // Not enough space in current chunk, need to swap it
            self.swap_buffer();
        }

        assert!(self.remaining_mut() >= src.remaining());

        while src.has_remaining() {
            let l;

            unsafe {
                let s = src.bytes();
                let d = self.bytes_mut();
                l = cmp::min(s.len(), d.len());

                ptr::copy_nonoverlapping(
                    s.as_ptr(),
                    d.as_mut_ptr() as *mut u8,
                    l);
            }

            src.advance(l);
            unsafe { self.advance_mut(l); }
        }
    }

    /// Override default impl to allow for large slices to be put in at a time
    fn put_slice(&mut self, src: &[u8]) {
        assert!(src.remaining() <= BUFFER_SIZE, "src too big for buffering");
        let mut off = 0;

        if self.remaining_mut() < src.len() {
            // Not enough space in current chunk, need to swap it
            self.swap_buffer();
        }

        assert!(self.remaining_mut() >= src.len(), "EncodeBuffer trying to write too big of a slice, len: {}", src.len());

        while off < src.len() {
            let cnt;

            unsafe {
                let dst = self.bytes_mut();
                cnt = cmp::min(dst.len(), src.len() - off);

                ptr::copy_nonoverlapping(
                    src[off..].as_ptr(),
                    dst.as_mut_ptr() as *mut u8,
                    cnt);

                off += cnt;
            }

            unsafe { self.advance_mut(cnt); }
        }
    }
}

/// Used to allow extraction of data frames in between inserting data
/// And can replace the underlying BufferChunk with new ones
pub struct DecodeBuffer {
    buffer: BufferChunk,
    write_offset: usize,
    read_offset: usize,
    next_frame_head: Option<FrameHead>,
}

impl DecodeBuffer {
    /// Creates a new DecodeBuffer from th given BufferChunk.
    pub fn new(buffer: BufferChunk) -> Self {
        DecodeBuffer {
            buffer,
            write_offset: 0,
            read_offset: 0,
            next_frame_head: None,
        }
    }

    /// Returns an IoVec of the writeable end of the buffer
    /// If something is written to the slice the advance writeable must be called after
    pub fn get_writeable(&mut self) -> Option<&mut IoVec> {
        if self.buffer.len() - self.write_offset < 128 {
            return None;
        }
        unsafe {
            Some(self.buffer.get_slice(self.write_offset, self.buffer.len()).into())
        }
    }

    /// Returns the length of the read-portion of the buffer
    pub fn len(&self) -> usize { self.write_offset - self.read_offset }

    /// Returns the length of the write-portion of the buffer
    pub fn remaining(&self) -> usize {
        self.buffer.len() - self.write_offset
    }

    /// Advances the write pointer by num_bytes
    pub fn advance_writeable(&mut self, num_bytes: usize) -> () {
        self.write_offset += num_bytes;
    }

    /// Get the current write-offset of the buffer
    pub fn get_write_offset(&self) -> usize {
        self.write_offset
    }

    /// Get the first 24 bytes of the DecodeBuffer, used for testing/debugging
    pub fn get_buffer_head(&mut self) -> &mut [u8] {
        unsafe {
            return self.buffer.get_slice(0, 24);
        }
    }

    /// Safely swaps self.buffer with other
    pub fn swap_buffer(&mut self, other: &mut BufferChunk) -> () {
        //println!("Swapping buffer");
        // Check if there's overflow in the buffer currently which needs to be copied
        if self.write_offset > self.read_offset {
            // We need to copy the bits from self.inner to other.inner before swapping
            let overflow_len = self.write_offset - self.read_offset;
            unsafe {
                other.get_slice(0, overflow_len).clone_from_slice(self.buffer.get_slice(self.read_offset, self.write_offset));
            }
            self.write_offset = overflow_len;
            self.read_offset = 0;
        } else {
            self.write_offset = 0;
            self.read_offset = 0;
        }
        // Swap the buffer chunks
        self.buffer.swap_buffer(other);
        other.lock();
    }


    /// Tries to decode one frame from the readable part of the buffer
    pub fn get_frame(&mut self) -> Option<Frame> {
        let readable_len = (self.write_offset - self.read_offset) as usize;
        if let Some(head) = &self.next_frame_head {
            if readable_len >= head.content_length() {
                unsafe {
                    let mut lease = self.buffer.get_lease(self.read_offset, self.read_offset + head.content_length());
                    self.read_offset += head.content_length();
                    match head.frame_type() {
                        FrameType::Data => {
                            if let Ok(data) = Data::decode_from(lease) {
                                self.next_frame_head = None;
                                return Some(data);
                            } else {
                                println!("Failed to decode data");
                            }
                        }
                        FrameType::StreamRequest => {
                            if let Ok(data) = StreamRequest::decode_from(lease) {
                                self.next_frame_head = None;
                                return Some(data);
                            }
                        }
                        FrameType::Hello => {
                            //println!("decoding Hello");
                            if let Ok(hello) = Hello::decode_from(lease) {
                                self.next_frame_head = None;
                                return Some(hello);
                            } else {
                                println!("Failed to decode data");
                            }
                        }
                        _ => {
                            println!("Weird head");
                        }
                    }
                }
            }
        } else {
            if readable_len >= FRAME_HEAD_LEN {
                unsafe {
                    let slice = self.buffer.get_slice(self.read_offset, self.read_offset + FRAME_HEAD_LEN);
                    self.read_offset += FRAME_HEAD_LEN;
                    if let Ok(head) = FrameHead::decode_from(&mut slice.borrow()) {
                        if head.content_length() == 0 {
                            //println!("Content len 0");
                            match head.frame_type() {
                                FrameType::Bye => {
                                    //println!("returning Bye frame");
                                    return Some(Frame::Bye());
                                }
                                _ => {}
                            }
                        }
                        self.next_frame_head = Some(head);
                        return self.get_frame();
                    } else {
                        // We are lost in the buffer, this is very bad
                        println!("Buffer split-off but not starting at a FrameHead: {:?}", &slice);
                    }
                }
            }
        }
        return None;
    }
}

/// A ChunkLease is a Byte-buffer which locks the BufferChunk which the ChunkLease is a lease from.
/// Can be used both as Buf of BufMut depending on what is needed.
#[derive(Debug)]
pub struct ChunkLease {
    content: &'static mut [u8],
    written: usize,
    read: usize,
    capacity: usize,
    lock: Arc<u8>,
}

impl ChunkLease {
    /// Creates a new ChunkLease from a static byte-slice with already written bytes and a `lock`.
    pub fn new(content: &'static mut [u8], lock: Arc<u8>) -> ChunkLease {
        let capacity = content.len();
        let written = content.len();
        ChunkLease {
            content,
            written,
            capacity,
            lock,
            read: 0,
        }
    }

    /// Creates a new ChunkLease with available space and a `lock`
    pub fn new_unused(content: &'static mut [u8], lock: Arc<u8>) -> ChunkLease {
        let capacity = content.len();
        ChunkLease {
            content,
            written: 0,
            capacity,
            lock,
            read: 0,
        }
    }

    /// Creates an empty ChunkLease, used as auxiliary method in test-cases
    pub fn empty() -> ChunkLease {
        static mut slice: [u8; 1] = [0];
        unsafe {
            let pointer: &'static mut [u8] = &mut slice;
            ChunkLease {
                read: 0,
                content: pointer,
                written: 0,
                capacity: 0,
                lock: Arc::new(0),
            }
        }
    }

    /// This inserts a FrameHead at the head of the Chunklease, the ChunkLease should be padded manually
    /// before this method is invoked, i.e. it does not create space for the head on its own.
    pub(crate) fn insert_head(&mut self, head: FrameHead) {
        // Store the write pointer
        let written = self.written;
        // Move write-pointer to the front of the buffer:
        self.written = 0;
        // Encode the into self
        head.encode_into(self);
        // Restore write-pointer
        self.written = written;
    }
}

impl Buf for ChunkLease {
    fn remaining(&self) -> usize {
        self.capacity - self.read
    }

    fn bytes(&self) -> &[u8] {
        //&(self.content).offset(self.read as isize);
        let ptr = self.content.as_ptr();
        unsafe {
            let offset_ptr = ptr.offset(self.read as isize);
            return std::slice::from_raw_parts(offset_ptr, self.capacity - self.read);
        }
    }

    fn advance(&mut self, cnt: usize) {
        self.read += cnt;
        //unsafe {self.content.advance_mut(cnt);}
    }
}

impl BufMut for ChunkLease {
    fn remaining_mut(&self) -> usize {
        self.remaining()
    }

    unsafe fn advance_mut(&mut self, cnt: usize) {
        //self.advance(cnt);
        self.written += cnt;
    }

    fn bytes_mut(&mut self) -> &mut [MaybeUninit<u8>] {
        let ptr = self.content.as_mut_ptr();
        unsafe {
            let offset_ptr = ptr.offset(self.written as isize);
            return mem::transmute(std::slice::from_raw_parts_mut(offset_ptr, self.capacity - self.written));
        }
        //unsafe { mem::transmute(&mut *&(self.content).offset(self.written as isize)) }
    }
}

unsafe impl Send for ChunkLease {}
// Drop automatically implemented

#[cfg(test)]
mod tests {
    // This is very non-exhaustive testing, just a 

    use super::*;

    // This test instantiates an EncodeBuffer and writes the same string into it enough times that
    // the EncodeBuffer should overload multiple times and will have to try to free at least 1 Chunk
    #[test]
    fn encode_buffer_overload() {
        // Instantiate an encode buffer with default values
        let mut encode_buffer = EncodeBuffer::new();

        // Create a string that's bigger than the ENCODEBUFFER_MIN_REMAINING
        use std::string::ToString;
        let mut test_string = "".to_string();
        for i in 0..=ENCODEBUFFER_MIN_REMAINING + 1 {
            // Make sure the string isn't just the same char over and over again,
            // Means we test for some more correctness in buffers
            test_string.push((i.to_string()).chars().next().unwrap());
        }
        let mut cnt = 0;

        // Make sure we churn through all the initial buffers.
        for _ in 0..=crate::net::buffer_pool::INITIAL_BUFFER_LEN + 1 {
            // Make sure we fill up a chunk per iteration:
            for _ in 0..=(BUFFER_SIZE / ENCODEBUFFER_MIN_REMAINING) + 1 {
                encode_buffer.put_slice(test_string.clone().as_bytes());
                let chunk = encode_buffer.get_chunk();
                assert_eq!(chunk.unwrap().content, test_string.as_bytes(), "cnt = {}", cnt);
                cnt += 1;
            }
        }

        // Check the buffer pool sizes
        assert_eq!(encode_buffer.buffer_pool.get_pool_sizes(),
                   (crate::net::buffer_pool::INITIAL_BUFFER_LEN, // Total allocated
                    0, // Available
                    crate::net::buffer_pool::INITIAL_BUFFER_LEN - 1)); // Returned
    }

    #[test]
    #[should_panic(expected = "src too big for buffering")]
    fn encode_buffer_panic() {
        let mut encode_buffer = EncodeBuffer::new();
        use std::string::ToString;
        let mut test_string = "".to_string();
        for i in 0..=BUFFER_SIZE + 10 {
            // Make sure the string isn't just the same char over and over again,
            // Means we test for some more correctness in buffers
            test_string.push((i.to_string()).chars().next().unwrap());
        }
        encode_buffer.put(test_string.as_bytes());
    }
}
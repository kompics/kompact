use super::*;
use crate::net::{buffer_pool::BufferPool, frames};
use bytes::{Buf, BufMut};
use core::{cmp, mem, ptr};
use std::{io::Cursor, mem::MaybeUninit, sync::Arc};
use std::alloc::{alloc, dealloc, Layout};

const FRAME_HEAD_LEN: usize = frames::FRAME_HEAD_LEN as usize;
const BUFFER_SIZE: usize = 1000 * 64;
// Assume 64 byte cache lines -> 1000 cache lines per chunk
const ENCODEBUFFER_MIN_REMAINING: usize = 64; // Always have at least a full cache line available

/// Required methods for a Chunk
pub trait Chunk {
    /// Returns a mutable pointer to the underlying buffer
    fn as_mut_ptr(&mut self) -> *mut u8;
    /// Returns the length of the chunk
    fn len(&self) -> usize;
}

/// A Default Kompact Chunk
pub(crate) struct DefaultChunk {
    chunk: *mut u8,
    len: usize,
}

impl DefaultChunk {
    pub fn new() -> DefaultChunk {
        unsafe {
            let layout = Layout::new::<[u8; BUFFER_SIZE]>();
            let chunk = alloc(layout);
            DefaultChunk { chunk, len: BUFFER_SIZE }
        }
    }
}

impl Chunk for DefaultChunk {
    fn as_mut_ptr(&mut self) -> *mut u8 {
        self.chunk
    }

    fn len(&self) -> usize {
        self.len
    }
}

/// BufferChunk is a lockable pinned byte-slice
/// All modifications to the Chunk goes through the get_slice method
pub struct BufferChunk {
    chunk: *mut dyn Chunk,
    ref_count: Arc<u8>,
    locked: bool,
}

impl Drop for BufferChunk {
    fn drop(&mut self) {
        unsafe {
            let chunk = Box::from_raw(self.chunk);
            std::mem::drop(chunk);
        }
    }
}

unsafe impl Send for BufferChunk {}

impl BufferChunk {
    /// Allocate a new Default BufferChunk
    pub fn new() -> Self {
        BufferChunk {
            chunk: Box::into_raw(Box::new(DefaultChunk::new())),
            ref_count: Arc::new(0),
            locked: false,
        }
    }

    /// Creates a BufferChunk using the given Chunk
    pub fn from_chunk(raw_chunk: *mut dyn Chunk) -> Self {
        BufferChunk {
            chunk: raw_chunk,
            ref_count: Arc::new(0),
            locked: false,
        }
    }

    /// Get the length of the BufferChunk
    pub fn len(&self) -> usize {
        unsafe { Chunk::len(&*self.chunk) }
    }

    /// Return a pointer to a subslice of the chunk
    pub unsafe fn get_slice(&mut self, from: usize, to: usize) -> &'static mut [u8] {
        assert!(from < to && to <= self.len() && !self.locked);
        let ptr = (&mut *self.chunk).as_mut_ptr();
        let offset_ptr = ptr.offset(from as isize);
        std::slice::from_raw_parts_mut(offset_ptr, to - from)
    }

    /// Swaps the pointers of the two buffers such that they effectively switch place
    pub fn swap_buffer(&mut self, other: &mut BufferChunk) -> () {
        std::mem::swap(self, other);
    }

    /// Returns a ChunkLease pointing to the subslice between `from` and `to`of the BufferChunk
    /// the returned lease is a consumable Buf.
    pub fn get_lease(&mut self, from: usize, to: usize) -> ChunkLease {
        let lock = self.get_lock();
        unsafe { ChunkLease::new(self.get_slice(from, to), lock) }
    }

    /// Returns a ChunkLease pointing to the subslice between `from` and `to`of the BufferChunk
    /// the returned lease is a writable BufMut.
    pub fn get_free_lease(&mut self, from: usize, to: usize) -> ChunkLease {
        let lock = self.get_lock();
        unsafe { ChunkLease::new_unused(self.get_slice(from, to), lock) }
    }

    /// Clones the lock, the BufferChunk will be locked until all given locks are deallocated
    pub fn get_lock(&self) -> Arc<u8> {
        self.ref_count.clone()
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

/// Instantiated Writer/Encoder for EncodeBuffer
/// Provides the BufMut interface over an EnodeBuffer ensuring that the EncodeBuffer will be aligned
/// after the BufferEncoder has been dropped.
pub struct BufferEncoder<'a> {
    encode_buffer: &'a mut EncodeBuffer,
}

impl<'a> BufferEncoder<'a> {
    pub fn new(encode_buffer: &'a mut EncodeBuffer) -> Self {
        // Check alignment, make sure we start aligned:
        assert_eq!(
            encode_buffer.get_write_offset(),
            encode_buffer.get_read_offset()
        );
        BufferEncoder { encode_buffer }
    }

    pub(crate) fn pad(&mut self, cnt: usize) {
        unsafe {
            self.advance_mut(cnt);
        }
    }

    pub fn get_chunk_lease(&mut self) -> Option<ChunkLease> {
        self.encode_buffer.get_chunk_lease()
    }
}

/// Make sure the read pointer is caught up to the write pointer for the next encoding.
impl<'a> Drop for BufferEncoder<'a> {
    fn drop(&mut self) {
        self.encode_buffer.read_offset = self.encode_buffer.write_offset;
    }
}

impl<'a> BufMut for BufferEncoder<'a> {
    fn remaining_mut(&self) -> usize {
        self.encode_buffer.remaining()
    }

    unsafe fn advance_mut(&mut self, cnt: usize) {
        self.encode_buffer.write_offset += cnt;
        if self.encode_buffer.remaining() < ENCODEBUFFER_MIN_REMAINING {
            self.encode_buffer.swap_buffer();
        }
    }

    fn bytes_mut(&mut self) -> &mut [MaybeUninit<u8>] {
        unsafe {
            let ptr = (&mut *self.encode_buffer.buffer.chunk).as_mut_ptr();
            let offset_ptr = ptr.offset(self.encode_buffer.write_offset as isize);
            return mem::transmute(std::slice::from_raw_parts_mut(
                offset_ptr,
                self.encode_buffer.buffer.len() - self.encode_buffer.write_offset,
            ));
        }
    }

    fn put<T: super::Buf>(&mut self, mut src: T)
    where
        Self: Sized,
    {
        assert!(src.remaining() <= BUFFER_SIZE, "src too big for buffering");
        if self.remaining_mut() < src.remaining() {
            // Not enough space in current chunk, need to swap it
            self.encode_buffer.swap_buffer();
        }

        assert!(self.remaining_mut() >= src.remaining());

        while src.has_remaining() {
            let l;
            unsafe {
                let s = src.bytes();
                let d = self.bytes_mut();
                l = cmp::min(s.len(), d.len());
                ptr::copy_nonoverlapping(s.as_ptr(), d.as_mut_ptr() as *mut u8, l);
            }
            src.advance(l);
            unsafe {
                self.advance_mut(l);
            }
        }
    }

    /// Override default impl to allow for large slices to be put in at a time
    fn put_slice(&mut self, src: &[u8]) {
        assert!(src.remaining() <= BUFFER_SIZE, "src too big for buffering");
        if self.remaining_mut() < src.len() {
            // Not enough space in current chunk, need to swap it
            self.encode_buffer.swap_buffer();
        }

        assert!(
            self.remaining_mut() >= src.len(),
            "EncodeBuffer trying to write too big of a slice, len: {}",
            src.len()
        );
        unsafe {
            let dst = self.bytes_mut();
            ptr::copy_nonoverlapping(src.as_ptr(), dst.as_mut_ptr() as *mut u8, src.len());
            self.advance_mut(src.len());
        }
    }
}

/// Structure used by components to continuously extract leases from BufferChunks swapped with the internal/private BufferPool
/// Using get_buffer_encoder() we get a BufferEncoder which implements BufMut interface for the underlying Chunks
/// The BufferEncoder uses its lifetime and Drop impl to give compile-time safety for consecutive encoding attempts
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
        if let Some(buffer) = buffer_pool.get_buffer() {
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

    pub fn advance(&mut self, cnt: usize) -> () {
        self.read_offset += cnt;
    }

    pub fn get_buffer_encoder(&mut self) -> BufferEncoder {
        BufferEncoder::new(self)
    }

    pub fn get_write_offset(&self) -> usize {
        self.write_offset
    }

    pub fn get_read_offset(&self) -> usize {
        self.read_offset
    }

    /// Extracts the bytes between the read-pointer and the write-pointer and advances the read-pointer
    /// Ensures there's a minimum of `ENCODEBUFFER_MIN_REMAINING` left in the current buffer which minimizes overflow during swap
    pub(crate) fn get_chunk_lease(&mut self) -> Option<ChunkLease> {
        let cnt = self.write_offset - self.read_offset;
        if cnt > 0 {
            self.read_offset += cnt;
            let lease = self
                .buffer
                .get_lease(self.write_offset - cnt, self.write_offset);

            if self.remaining() < ENCODEBUFFER_MIN_REMAINING {
                self.swap_buffer();
            }

            return Some(lease);
        }
        None
    }

    /// Returns the remaining length of the current BufferChunk.
    pub(crate) fn remaining(&self) -> usize {
        self.buffer.len() - self.write_offset
    }

    /// Swap the current buffer with a fresh one from the private buffer_pool
    /// Copies any unread overflow and sets the write pointer accordingly
    pub(crate) fn swap_buffer(&mut self) {
        let overflow = self.get_chunk_lease();
        if let Some(mut new_buffer) = self.buffer_pool.get_buffer() {
            self.buffer.swap_buffer(&mut new_buffer);
            self.read_offset = 0;
            self.write_offset = 0;
            if let Some(chunk) = overflow {
                let len = chunk.bytes().len();
                assert!(len < self.remaining());
                unsafe {
                    ptr::copy_nonoverlapping(
                        chunk.bytes().as_ptr(),
                        self.buffer.get_slice(0, len).as_mut_ptr(),
                        len,
                    );
                    self.write_offset = chunk.bytes().len();
                }
            }
            new_buffer.lock();
            // new_buffer has been swapped in-place, return it to the pool
            self.buffer_pool.return_buffer(new_buffer);
        } else {
            panic!("Trying to swap buffer but no free buffers found in the pool!")
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

    /// Returns an Write compatible slice of the writeable end of the buffer
    /// If something is written to the slice advance writeable must be called after
    pub fn get_writeable(&mut self) -> Option<&mut [u8]> {
        if self.buffer.len() - self.write_offset < 128 {
            return None;
        }
        unsafe { Some(self.buffer.get_slice(self.write_offset, self.buffer.len())) }
    }

    /// Returns the length of the read-portion of the buffer
    pub fn len(&self) -> usize {
        self.write_offset - self.read_offset
    }

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

    /// Get the current read-offset of the buffer
    pub fn get_read_offset(&self) -> usize {
        self.read_offset
    }

    /// Get the first 24 bytes of the DecodeBuffer, used for testing/debugging
    /// get_slice does the bounds checking
    pub fn get_buffer_head(&mut self) -> &mut [u8] {
        unsafe {
            return self.buffer.get_slice(0, 24);
        }
    }

    /// Swaps the underlying buffer in place with other
    /// Afterwards `other` owns the used up bytes and is locked.
    pub fn swap_buffer(&mut self, other: &mut BufferChunk) -> () {
        assert!(!other.locked); // Must not try to swap in a locked buffer

        let overflow = self.get_overflow();

        self.buffer.swap_buffer(other);
        self.read_offset = 0;
        self.write_offset = 0;
        if let Some(chunk) = overflow {
            let len = chunk.bytes().len();
            assert!(len < self.remaining()); // the overflow must not exceed the new buffers capacity
            unsafe {
                ptr::copy_nonoverlapping(
                    chunk.bytes().as_ptr(),
                    self.buffer.get_slice(0, len).as_mut_ptr(),
                    len,
                );
                self.write_offset = chunk.bytes().len();
            }
        }
        // other has been swapped in-place, can be returned to the pool
        other.lock();
    }

    /// Tries to decode one frame from the readable part of the buffer
    pub fn get_frame(&mut self) -> Result<Frame, FramingError> {
        let readable_len = (self.write_offset - self.read_offset) as usize;
        if let Some(head) = &self.next_frame_head {
            if readable_len >= head.content_length() {
                drop(head);
                let head = self.next_frame_head.take().unwrap();
                let lease = self
                    .buffer
                    .get_lease(self.read_offset, self.read_offset + head.content_length());
                self.read_offset += head.content_length();
                match head.frame_type() {
                    // Frames with empty bodies should be handled in frame-head decoding below.
                    FrameType::Data => {
                        if let Ok(data) = Data::decode_from(lease) {
                            return Ok(data);
                        }
                        return Err(FramingError::InvalidFrame);
                    }
                    FrameType::StreamRequest => {
                        if let Ok(data) = StreamRequest::decode_from(lease) {
                            return Ok(data);
                        }
                        return Err(FramingError::InvalidFrame);
                    }
                    FrameType::Hello => {
                        if let Ok(hello) = Hello::decode_from(lease) {
                            return Ok(hello);
                        }
                        return Err(FramingError::InvalidFrame);
                    }
                    FrameType::Start => {
                        if let Ok(start) = Start::decode_from(lease) {
                            return Ok(start);
                        }
                        return Err(FramingError::InvalidFrame);
                    }
                    FrameType::Ack => {
                        if let Ok(ack) = Ack::decode_from(lease) {
                            return Ok(ack);
                        }
                        return Err(FramingError::InvalidFrame);
                    }
                    _ => {
                        return Err(FramingError::UnsupportedFrameType);
                    }
                }
            }
        } else {
            if readable_len >= FRAME_HEAD_LEN {
                unsafe {
                    let slice = self
                        .buffer
                        .get_slice(self.read_offset, self.read_offset + FRAME_HEAD_LEN);
                    self.read_offset += FRAME_HEAD_LEN;
                    let mut cursor = Cursor::new(slice);
                    let head = FrameHead::decode_from(&mut cursor)?;
                    if head.content_length() == 0 {
                        match head.frame_type() {
                            // Frames without content match here for expediency, Decoder doesn't allow 0 length.
                            FrameType::Bye => {
                                return Ok(Frame::Bye());
                            }
                            _ => {}
                        }
                    } else {
                        self.next_frame_head = Some(head);
                        return self.get_frame();
                    }
                }
            }
        }
        return Err(FramingError::NoData);
    }

    pub(crate) fn get_overflow(&mut self) -> Option<ChunkLease> {
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
        static mut SLICE: [u8; 1] = [0];
        unsafe {
            let pointer: &'static mut [u8] = &mut SLICE;
            ChunkLease {
                read: 0,
                content: pointer,
                written: 0,
                capacity: 0,
                lock: Arc::new(0),
            }
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// This inserts a FrameHead at the head of the Chunklease, the ChunkLease should be padded manually
    /// before this method is invoked, i.e. it does not create space for the head on its own
    /// Proper framing thus requires 1. pad(), 2 serialise into DecodeBuffer, 3. get_chunk_lease, 4. insert_head
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
        let slice: &[u8] = &*self.content;
        &slice[self.read..self.capacity]
    }

    fn advance(&mut self, cnt: usize) {
        self.read += cnt;
    }
}

impl BufMut for ChunkLease {
    fn remaining_mut(&self) -> usize {
        self.capacity - self.written
    }

    unsafe fn advance_mut(&mut self, cnt: usize) {
        self.written += cnt;
    }

    fn bytes_mut(&mut self) -> &mut [MaybeUninit<u8>] {
        unsafe {
            let slice: &mut [u8] = &mut *self.content;
            mem::transmute(&mut slice[self.written..self.capacity])
        }
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
        {
            let buffer_encoder = &mut EncodeBuffer::get_buffer_encoder(&mut encode_buffer);

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
                    buffer_encoder.put_slice(test_string.clone().as_bytes());
                    let chunk = buffer_encoder.get_chunk_lease();
                    assert_eq!(
                        chunk.unwrap().bytes(),
                        test_string.as_bytes(),
                        "cnt = {}",
                        cnt
                    );
                    cnt += 1;
                }
            }
        }
        // Check the buffer pool sizes
        assert_eq!(
            encode_buffer.buffer_pool.get_pool_sizes(),
            (
                crate::net::buffer_pool::INITIAL_BUFFER_LEN, // Total allocated
                0,                                           // Available
                crate::net::buffer_pool::INITIAL_BUFFER_LEN - 1
            )
        ); // Returned
    }

    #[test]
    #[should_panic(expected = "src too big for buffering")]
    fn encode_buffer_panic() {
        let mut encode_buffer = EncodeBuffer::new();
        let buffer_encoder = &mut EncodeBuffer::get_buffer_encoder(&mut encode_buffer);
        use std::string::ToString;
        let mut test_string = "".to_string();
        for i in 0..=BUFFER_SIZE + 10 {
            test_string.push((i.to_string()).chars().next().unwrap());
        }
        buffer_encoder.put(test_string.as_bytes());
    }

    #[test]
    fn aligned_after_drop() {
        let mut encode_buffer = EncodeBuffer::new();
        {
            let buffer_encoder = &mut EncodeBuffer::get_buffer_encoder(&mut encode_buffer);
            buffer_encoder.put_i32(16);
        }
        // Check the buffer pool sizes
        assert_eq!(encode_buffer.write_offset, encode_buffer.read_offset);
        assert_eq!(encode_buffer.write_offset, 4);
    }
}

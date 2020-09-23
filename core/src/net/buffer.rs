use super::*;
use crate::{
    messaging::DispatchEnvelope,
    net::{
        buffer_pool::{BufferPool, ChunkAllocator},
        frames,
    },
};
use bytes::{Buf, BufMut};
use core::{cmp, ptr};
use hocon::Hocon;
use std::{
    fmt::{Debug, Formatter},
    io::Cursor,
    mem::MaybeUninit,
    sync::Arc,
};

const FRAME_HEAD_LEN: usize = frames::FRAME_HEAD_LEN as usize;

/// The configuration for the network buffers
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct BufferConfig {
    /// Specifies the length of `BufferChunk`s
    pub(crate) chunk_size: usize,
    /// Specifies the amount of `BufferChunk` a `BufferPool` will pre-allocate on creation
    pub(crate) initial_pool_count: usize,
    /// Specifies the maximum amount of `BufferChunk`s individual `BufferPool`s will allocate
    pub(crate) max_pool_count: usize,
    /// Minimum number of bytes an `EncodeBuffer` must have available before serialisation into it
    pub(crate) encode_min_remaining: usize,
}

impl BufferConfig {
    /// Create a new BufferConfig with default values which may be overwritten
    pub fn new() -> Self {
        BufferConfig {
            chunk_size: 128 * 1000,   // 128Kb chunks
            initial_pool_count: 2,    // 256Kb initial/minimum BufferPools
            max_pool_count: 1000,     // 128Mb maximum BufferPools
            encode_min_remaining: 64, // typical L1 cache line size
        }
    }

    /// Sets the BufferConfigs chunk_size to the given value
    pub fn chunk_size(&mut self, size: usize) -> () {
        self.chunk_size = size;
    }
    /// Sets the BufferConfigs initial_pool_count to the given value
    pub fn initial_pool_count(&mut self, count: usize) -> () {
        self.initial_pool_count = count;
    }
    /// Sets the BufferConfigs max_pool_count to the given value
    pub fn max_pool_count(&mut self, count: usize) -> () {
        self.max_pool_count = count;
    }
    /// Sets the BufferConfigs encode_min_remaining to the given value
    pub fn encode_min_remaining(&mut self, size: usize) -> () {
        self.encode_min_remaining = size;
    }

    /// Tries to deserialize a BufferConfig from the specified in the given `config`.
    /// Returns a default BufferConfig if it fails to read from the config.
    pub fn from_config(config: &Hocon) -> BufferConfig {
        let mut buffer_config = BufferConfig::new();
        if let Some(chunk_size) = config["buffer_config"]["chunk_size"].as_i64() {
            buffer_config.chunk_size = chunk_size as usize;
        }
        if let Some(initial_pool_count) = config["buffer_config"]["initial_pool_count"].as_i64() {
            buffer_config.initial_pool_count = initial_pool_count as usize;
        }
        if let Some(max_pool_count) = config["buffer_config"]["max_pool_count"].as_i64() {
            buffer_config.max_pool_count = max_pool_count as usize;
        }
        if let Some(encode_min_remaining) = config["buffer_config"]["encode_min_remaining"].as_i64()
        {
            buffer_config.encode_min_remaining = encode_min_remaining as usize;
        }
        buffer_config.validate();
        buffer_config
    }

    /// Performs basic sanity checks on the config parameters
    pub fn validate(&self) {
        if self.initial_pool_count > self.max_pool_count {
            panic!("initial_pool_count may not be greater than max_pool_count")
        }
        if self.chunk_size <= self.encode_min_remaining {
            panic!("chunk_size must be greater than encode_min_remaining")
        }
        if self.chunk_size < 128 {
            panic!("chunk_size smaller than 128 is not allowed")
        }
    }
}

/// Required methods for a Chunk
pub trait Chunk {
    /// Returns a mutable pointer to the underlying buffer
    fn as_mut_ptr(&mut self) -> *mut u8;
    /// Returns the length of the chunk
    fn len(&self) -> usize;
    /// Returns `true` if the length of the chunk is 0
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A Default Kompact Chunk
pub(crate) struct DefaultChunk {
    chunk: Box<[u8]>,
}

impl DefaultChunk {
    pub fn new(size: usize) -> DefaultChunk {
        let v = vec![0u8; size];
        let slice = v.into_boxed_slice();
        DefaultChunk { chunk: slice }
    }
}

impl Chunk for DefaultChunk {
    fn as_mut_ptr(&mut self) -> *mut u8 {
        self.chunk.as_mut_ptr()
    }

    fn len(&self) -> usize {
        self.chunk.len()
    }

    fn is_empty(&self) -> bool {
        self.chunk.is_empty()
    }
}

/// BufferChunk is a lockable pinned byte-slice
/// All modifications to the Chunk goes through the get_slice method
pub struct BufferChunk {
    chunk: *mut dyn Chunk,
    ref_count: Arc<u8>,
    locked: bool,
}

impl Debug for BufferChunk {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BufferChunk")
            .field("Lock", &self.locked)
            .field("Length", &self.len())
            .field("RefCount", &Arc::strong_count(&self.ref_count))
            .finish()
    }
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
    pub fn new(size: usize) -> Self {
        BufferChunk {
            chunk: Box::into_raw(Box::new(DefaultChunk::new(size))),
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

    pub fn is_empty(&self) -> bool {
        unsafe { Chunk::is_empty(&*self.chunk) }
    }

    /// Return a pointer to a subslice of the chunk
    unsafe fn get_slice(&mut self, from: usize, to: usize) -> &'static mut [u8] {
        assert!(from < to && to <= self.len() && !self.locked);
        let ptr = (&mut *self.chunk).as_mut_ptr();
        let offset_ptr = ptr.add(from);
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

    /// Returns true if this BufferChunk is available again
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
        if self.encode_buffer.write_offset > self.encode_buffer.buffer.len() {
            panic!(
                "Fatal error in buffers, write-pointer exceeding buffer length,\
            this should not happen"
            );
        }
    }

    fn bytes_mut(&mut self) -> &mut [MaybeUninit<u8>] {
        unsafe {
            let ptr = (&mut *self.encode_buffer.buffer.chunk).as_mut_ptr();
            let offset_ptr = ptr.add(self.encode_buffer.write_offset);
            &mut *(std::slice::from_raw_parts_mut(
                offset_ptr,
                self.encode_buffer.buffer.len() - self.encode_buffer.write_offset,
            ) as *mut [u8] as *mut [std::mem::MaybeUninit<u8>])
        }
    }

    fn put<T: super::Buf>(&mut self, mut src: T)
    where
        Self: Sized,
    {
        assert!(
            src.remaining() <= self.encode_buffer.buffer.len(),
            "src too big for buffering"
        );
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
        assert!(
            src.remaining() <= self.encode_buffer.buffer.len(),
            "src too big for buffering"
        );
        if self.remaining_mut() < src.len() {
            // Not enough space in current chunk, need to swap it
            self.encode_buffer.swap_buffer();
        }

        assert!(
            self.remaining_mut() >= src.len(),
            "EncodeBuffer trying to write too big, slice len: {}, Chunk len: {}, Remaining: {}",
            src.len(),
            self.encode_buffer.buffer.len(),
            self.remaining_mut()
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
    dispatcher_ref: Option<DispatcherRef>,
    min_remaining: usize,
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
                min_remaining: config.encode_min_remaining,
            }
        } else {
            panic!("Couldn't initialize EncodeBuffer");
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
                min_remaining: config.encode_min_remaining,
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

    /*
    /// Deallocates unlocked Buffers in the pool and returns the locked BufferChunks
    pub(crate) fn destroy(self) -> (BufferChunk, BufferPool, Option<DispatcherRef>) {
        (self.buffer, self.buffer_pool, self.dispatcher_ref)
    } */

    pub fn get_read_offset(&self) -> usize {
        self.read_offset
    }

    /// Extracts the bytes between the read-pointer and the write-pointer and advances the read-pointer
    /// Ensures there's a configurable-minimum left in the current buffer, minimizes overflow during swap
    pub(crate) fn get_chunk_lease(&mut self) -> Option<ChunkLease> {
        let cnt = self.write_offset - self.read_offset;
        if cnt > 0 {
            self.read_offset += cnt;
            let lease = self
                .buffer
                .get_lease(self.write_offset - cnt, self.write_offset);

            if self.remaining() < self.min_remaining {
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

impl Drop for EncodeBuffer {
    fn drop(&mut self) {
        // If the buffer was spawned with a dispatcher_ref all locked buffers are sent to the dispatcher for garbage collection
        if let Some(dispatcher_ref) = self.dispatcher_ref.take() {
            // Make sure that the active_buffer is fresh and whatever was in there is locked and stored in the return queue
            self.swap_buffer();
            // Drain the returned queue and send locked buffers to the dispatcher
            for mut trash in self.buffer_pool.drain_returned() {
                if !trash.free() {
                    dispatcher_ref.tell(DispatchEnvelope::LockedChunk(trash));
                }
            }
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

    /// Returns `true` if the buffer's read-portion has a length of 0.
    pub fn is_empty(&self) -> bool {
        self.write_offset == self.read_offset
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
        unsafe { self.buffer.get_slice(0, 24) }
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
                let head = self.next_frame_head.take().unwrap();
                let lease = self
                    .buffer
                    .get_lease(self.read_offset, self.read_offset + head.content_length());
                self.read_offset += head.content_length();
                match head.frame_type() {
                    // Frames with empty bodies should be handled in frame-head decoding below.
                    FrameType::Data => {
                        if let Ok(data) = Data::decode_from(lease) {
                            Ok(data)
                        } else {
                            Err(FramingError::InvalidFrame)
                        }
                    }
                    FrameType::StreamRequest => {
                        if let Ok(data) = StreamRequest::decode_from(lease) {
                            Ok(data)
                        } else {
                            Err(FramingError::InvalidFrame)
                        }
                    }
                    FrameType::Hello => {
                        if let Ok(hello) = Hello::decode_from(lease) {
                            Ok(hello)
                        } else {
                            Err(FramingError::InvalidFrame)
                        }
                    }
                    FrameType::Start => {
                        if let Ok(start) = Start::decode_from(lease) {
                            Ok(start)
                        } else {
                            Err(FramingError::InvalidFrame)
                        }
                    }
                    FrameType::Ack => {
                        if let Ok(ack) = Ack::decode_from(lease) {
                            Ok(ack)
                        } else {
                            Err(FramingError::InvalidFrame)
                        }
                    }
                    _ => Err(FramingError::UnsupportedFrameType),
                }
            } else {
                Err(FramingError::NoData)
            }
        } else if readable_len >= FRAME_HEAD_LEN {
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
                        FrameType::Bye => Ok(Frame::Bye()),
                        _ => Err(FramingError::NoData),
                    }
                } else {
                    self.next_frame_head = Some(head);
                    self.get_frame()
                }
            }
        } else {
            Err(FramingError::NoData)
        }
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
            &mut *(&mut slice[self.written..self.capacity] as *mut [u8]
                as *mut [std::mem::MaybeUninit<u8>])
        }
    }
}

unsafe impl Send for ChunkLease {}
// Drop automatically implemented

#[cfg(test)]
mod tests {
    // This is very non-exhaustive testing, just basic sanity checks

    use super::*;
    use crate::prelude::NetMessage;
    use hocon::HoconLoader;
    use std::{borrow::Borrow, time::Duration};

    #[test]
    #[should_panic(expected = "initial_pool_count may not be greater than max_pool_count")]
    fn invalid_pool_counts_config_validation() {
        let hocon = HoconLoader::new()
            .load_str(
                r#"{
            buffer_config {
                chunk_size: 64,
                initial_pool_count: 3,
                max_pool_count: 2,
                encode_min_remaining: 2,
                }
            }"#,
            )
            .unwrap()
            .hocon();
        // This should succeed
        let cfg = hocon.unwrap();

        // Validation should panic
        let _ = BufferConfig::from_config(&cfg);
    }

    #[test]
    #[should_panic(expected = "chunk_size must be greater than encode_min_remaining")]
    fn invalid_encode_min_remaining_validation() {
        // The BufferConfig should panic because encode_min_remain too high
        let mut buffer_config = BufferConfig::new();
        buffer_config.chunk_size(128);
        buffer_config.encode_min_remaining(128);
        buffer_config.validate();
    }

    // This test instantiates an EncodeBuffer and writes the same string into it enough times that
    // the EncodeBuffer should overload multiple times and will have to succeed in reusing >=1 Chunk
    #[test]
    fn encode_buffer_overload_reuse_default_config() {
        let buffer_config = BufferConfig::new();
        let encode_buffer = encode_buffer_overload_reuse(&buffer_config);
        // Check the buffer pool sizes
        assert_eq!(
            encode_buffer.buffer_pool.get_pool_sizes(),
            (
                buffer_config.initial_pool_count, // No additional has been allocated
                buffer_config.initial_pool_count - 1  // Currently in the pool
            )
        );
    }

    // As above, except we create a much larger BufferPool with larger Chunks manually configured
    #[test]
    fn encode_buffer_overload_reuse_manually_configured_large_buffers() {
        let mut buffer_config = BufferConfig::new();
        buffer_config.chunk_size(50000000);     // 50 MB chunk_size
        buffer_config.initial_pool_count(10);  // 500 MB pool init value
        buffer_config.max_pool_count(20);      // 1 GB pool max size
        buffer_config.encode_min_remaining(256);// 256 B min_remaining

        let encode_buffer = encode_buffer_overload_reuse(&buffer_config);
        assert_eq!(encode_buffer.buffer.len(), 50000000);
        // Check the buffer pool sizes
        assert_eq!(
            encode_buffer.buffer_pool.get_pool_sizes(),
            (
                10,     // No additional has been allocated
                10 - 1  // Currently in the pool
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
                initial_pool_count: 2,
                max_pool_count: 2,
                encode_min_remaining: 2,
                }
            }"#,
            )
            .unwrap()
            .hocon();
        let buffer_config = BufferConfig::from_config(&hocon.unwrap());
        // Ensure we successfully parsed the Config
        assert_eq!(buffer_config.encode_min_remaining, 2 as usize);
        let encode_buffer = encode_buffer_overload_reuse(&buffer_config);
        // Ensure that the
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

    // Creates an EncodeBuffer from the given config and uses the configured values to ensure
    // The pool is exhausted (or would be if no reuse) and reuses buffers at least once.
    fn encode_buffer_overload_reuse(buffer_config: &BufferConfig) -> EncodeBuffer {
        // Instantiate an encode buffer with default values
        let mut encode_buffer = EncodeBuffer::with_config(&buffer_config, &None);
        {
            let buffer_encoder = &mut EncodeBuffer::get_buffer_encoder(&mut encode_buffer);

            // Create a string that's bigger than the ENCODEBUFFER_MIN_REMAINING
            let mut test_string = "".to_string();
            for i in 0..=&buffer_config.encode_min_remaining + 1 {
                // Make sure the string isn't just the same char over and over again,
                // Means we test for some more correctness in buffers
                test_string.push((i.to_string()).chars().next().unwrap());
            }
            let mut cnt = 0;

            // Make sure we churn through all the initial buffers.
            for _ in 0..=&buffer_config.initial_pool_count + 1 {
                // Make sure we fill up a chunk per iteration:
                for _ in 0..=(&buffer_config.chunk_size / &buffer_config.encode_min_remaining) + 1 {
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
        encode_buffer
    }

    #[test]
    #[should_panic(expected = "src too big for buffering")]
    fn encode_buffer_panic() {
        // Instantiate an encode buffer with default values
        let buffer_config = BufferConfig::new();
        let mut encode_buffer = EncodeBuffer::with_config(&buffer_config, &None);
        let buffer_encoder = &mut EncodeBuffer::get_buffer_encoder(&mut encode_buffer);
        use std::string::ToString;
        let mut test_string = "".to_string();
        for i in 0..=buffer_config.chunk_size + 10 {
            test_string.push((i.to_string()).chars().next().unwrap());
        }
        buffer_encoder.put(test_string.as_bytes());
    }

    #[test]
    fn aligned_after_drop() {
        // Instantiate an encode buffer with default values
        let buffer_config = BufferConfig::new();
        let mut encode_buffer = EncodeBuffer::with_config(&buffer_config, &None);
        {
            let buffer_encoder = &mut EncodeBuffer::get_buffer_encoder(&mut encode_buffer);
            buffer_encoder.put_i32(16);
        }
        // Check the read and write pointers are aligned despite no read call
        assert_eq!(encode_buffer.write_offset, encode_buffer.read_offset);
        assert_eq!(encode_buffer.write_offset, 4);
    }

    #[derive(ComponentDefinition)]
    struct BufferTestActor {
        ctx: ComponentContext<BufferTestActor>,
        custom_buf: bool,
    }
    impl BufferTestActor {
        fn with_custom_buffer() -> BufferTestActor {
            BufferTestActor {
                ctx: ComponentContext::uninitialised(),
                custom_buf: true,
            }
        }

        fn without_custom_buffer() -> BufferTestActor {
            BufferTestActor {
                ctx: ComponentContext::uninitialised(),
                custom_buf: false,
            }
        }
    }
    impl Actor for BufferTestActor {
        type Message = ();

        fn receive_local(&mut self, _: Self::Message) -> Handled {
            Handled::Ok
        }

        fn receive_network(&mut self, _: NetMessage) -> Handled {
            Handled::Ok
        }
    }
    impl ComponentLifecycle for BufferTestActor {
        fn on_start(&mut self) -> Handled {
            if self.custom_buf {
                let mut buffer_config = BufferConfig::new();
                buffer_config.encode_min_remaining(30);
                buffer_config.max_pool_count(5);
                buffer_config.initial_pool_count(4);
                buffer_config.chunk_size(128);
                // Initialize the buffers
                self.ctx.borrow().init_buffers(Some(buffer_config), None);
            }
            // Use the Buffer
            let _ = self.ctx.actor_path().clone().tell_serialised(120, self);
            Handled::Ok
        }

        fn on_stop(&mut self) -> Handled {
            Handled::Ok
        }

        fn on_kill(&mut self) -> Handled {
            Handled::Ok
        }
    }
    fn buffer_config_testing_system() -> KompactSystem {
        let mut cfg = KompactConfig::new();
        let mut network_buffer_config = BufferConfig::new();
        network_buffer_config.chunk_size(512);
        network_buffer_config.initial_pool_count(2);
        network_buffer_config.max_pool_count(3);
        network_buffer_config.encode_min_remaining(10);
        cfg.load_config_str(
            r#"{
                buffer_config {
                    chunk_size: 256,
                    initial_pool_count: 3,
                    max_pool_count: 4,
                    encode_min_remaining: 20,
                    }
                }"#,
        );
        cfg.system_components(DeadletterBox::new, {
            NetworkConfig::with_buffer_config(
                "127.0.0.1:0".parse().expect("Address should work"),
                network_buffer_config,
            )
            .build()
        });
        cfg.build().expect("KompactSystem")
    }
    // This integration test sets up a KompactSystem with a Hocon BufferConfig,
    // then runs init_buffers on an Actor with different settings (in the on_start method of dummy),
    // and finally asserts that the actors buffers were set up using the on_start parameters.
    #[test]
    fn buffer_config_init_buffers_overrides_hocon_and_default() {
        let system = buffer_config_testing_system();
        let (dummy, _df) = system.create_and_register(BufferTestActor::with_custom_buffer);
        let dummy_f = system.register_by_alias(&dummy, "dummy");
        let _ = dummy_f.wait_expect(Duration::from_millis(1000), "dummy failed");
        system.start(&dummy);
        // TODO maybe we could do this a bit more reliable?
        thread::sleep(Duration::from_millis(100));

        // Read the buffer_len
        let mut buffer_len = 0;
        let mut buffer_write_pointer = 0;
        dummy.on_definition(|c| {
            if let Some(encode_buffer) = c.ctx.get_buffer_location().borrow().as_ref() {
                buffer_len = encode_buffer.buffer.len();
                buffer_write_pointer = encode_buffer.write_offset;
            }
        });
        // Assert that the buffer was initialized with parameter in the actors on_start method
        assert_eq!(buffer_len, 128);
        // Check that the buffer was used
        assert_ne!(buffer_write_pointer, 0);
    }

    #[test]
    fn buffer_config_hocon_overrides_default() {
        let system = buffer_config_testing_system();
        let (dummy, _df) = system.create_and_register(BufferTestActor::without_custom_buffer);
        let dummy_f = system.register_by_alias(&dummy, "dummy");
        let _ = dummy_f.wait_expect(Duration::from_millis(1000), "dummy failed");
        system.start(&dummy);
        // TODO maybe we could do this a bit more reliable?
        thread::sleep(Duration::from_millis(100));

        // Read the buffer_len
        let mut buffer_len = 0;
        let mut buffer_write_pointer = 0;
        dummy.on_definition(|c| {
            if let Some(encode_buffer) = c.ctx.get_buffer_location().borrow().as_ref() {
                buffer_len = encode_buffer.buffer.len();
                buffer_write_pointer = encode_buffer.write_offset;
            }
        });
        // Assert that the buffer was initialized with parameters in the hocon config string
        assert_eq!(buffer_len, 256);
        // Check that the buffer was used
        assert_ne!(buffer_write_pointer, 0);
    }
}

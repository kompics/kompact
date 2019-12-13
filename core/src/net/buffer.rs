use bytes::{BytesMut, BufMut, Buf, Bytes};
use iovec::IoVec;
use crate::net::frames::*;
use crate::net::frames;
use std::collections::VecDeque;
use std::pin::Pin;
use std::ops::{DerefMut, Deref};
use parking_lot::{Mutex, MutexGuard};
use std::sync::Arc;

pub const FRAME_HEAD_LEN: usize = frames::FRAME_HEAD_LEN as usize;
const BUFFER_SIZE: usize = 655355; // Idk what's a good number here: To be determined later



pub struct BufferChunk {
    chunk: Pin<Box<[u8; BUFFER_SIZE]>>,
    ref_count: Arc<u8>,
    locked: bool,
}

impl BufferChunk {
    pub fn new() -> Self {
        let mut slice = ([0u8; BUFFER_SIZE]);
        BufferChunk {
            chunk: Pin::new(Box::new(slice)),
            ref_count: Arc::new(0),
            locked: false,
        }
    }

    pub fn len(&self) -> usize {
        self.chunk.len()
    }

    /// Private function returning a pointer to a subslice of the chunk
    pub unsafe fn get_slice(&mut self, from: usize, to: usize) -> &mut [u8] {
        assert!(from < to && to <= self.len() && !self.locked);
        let ptr = self.chunk.as_mut_ptr();
        let offset_ptr = ptr.offset(from as isize);
        std::slice::from_raw_parts_mut(offset_ptr, to-from)
    }

    pub fn get_bytes(&mut self, from: usize, to: usize) -> Bytes {
        let slice = unsafe {
            self.get_slice(from, to)
        };
        let mut bytes_mut = BytesMut::with_capacity(to-from);
        bytes_mut.extend_from_slice(slice);
        return bytes_mut.freeze();
        //return bytes::Bytes::from(slice.to_vec())
    }

    pub fn get_lease(&mut self, from: usize, to: usize) -> ChunkLease<Bytes> {
        ChunkLease::new(self.get_bytes(from, to), self.get_lock())
    }

    pub fn get_lock(&self) -> Arc<u8> {
        self.ref_count.clone()
    }

    pub fn get_chunk_mut(&mut self) -> &mut Pin<Box<[u8; BUFFER_SIZE]>> {
        &mut self.chunk
    }

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

    pub fn lock(&mut self) -> () {
        self.locked = true;
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

impl DecodeBuffer{
    pub fn new(buffer: BufferChunk) -> Self {
        DecodeBuffer{
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
            return None
        }
        unsafe {
            Some(self.buffer.get_slice(self.write_offset, self.buffer.len()).into())
        }
    }

    /// Advances the write pointer by num_bytes
    pub fn advance_writeable(&mut self, num_bytes: usize) -> () {
        self.write_offset += num_bytes;
    }

    pub fn get_write_offset(&self) -> usize {
        self.write_offset
    }

    pub fn get_buffer_head(&mut self) -> &mut [u8] {
        unsafe {
            return self.buffer.get_slice(0,24);
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
        std::mem::swap(self.buffer.get_chunk_mut(),  other.get_chunk_mut());
        std::mem::swap(self.buffer.get_lock_mut(), other.get_lock_mut());
        other.lock();
    }


    /// Tries to decode one frame from the readable part of the buffer
    pub fn get_frame(&mut self) -> Option<Frame> {
        let readable_len = (self.write_offset-self.read_offset) as usize;
        if let Some(head) = &self.next_frame_head {
            if readable_len >= head.content_length() {
                unsafe {
                    let mut lease = self.buffer.get_lease(self.read_offset, self.read_offset+head.content_length());
                    self.read_offset += head.content_length();
                    match head.frame_type() {
                        FrameType::Data => {
                            if let Ok(data) = Data::decode_from(lease) {
                                self.next_frame_head = None;
                                return Some(data);
                            } else {
                                println!("Failed to decode data");
                            }
                        },
                        FrameType::StreamRequest => {
                            if let Ok(data) = StreamRequest::decode_from(lease) {
                                self.next_frame_head = None;
                                return Some(data);
                            }
                        },
                        _ => {
                            println!("Weird head");
                        }
                    }
                }
            }
        }
        else {
            if readable_len >= FRAME_HEAD_LEN {
                unsafe {
                    let mut bytes = self.buffer.get_bytes(self.read_offset, self.read_offset + FRAME_HEAD_LEN);
                    self.read_offset += FRAME_HEAD_LEN;
                    if let Ok(head) = FrameHead::decode_from(&mut bytes) {
                        self.next_frame_head = Some(head);
                        return self.get_frame();
                    } else {
                        // We are lost in the buffer, this is very bad
                        println!("Buffer split-off but not starting at a FrameHead");
                    }
                }
            }
        }
        return None;
    }
}

#[derive(Debug)]
pub struct ChunkLease<B: Buf + Sized> {
    content: B,
    lock: Arc<u8>,
}

impl<B: Buf + Sized> ChunkLease<B> {
    pub fn new(content: B, lock: Arc<u8>) -> ChunkLease<B> {
        ChunkLease{
            content,
            lock,
        }
    }
}

impl<B: Buf> Deref for ChunkLease<B> {
    type Target = B;

    fn deref(&self) -> &B {
        &self.content
    }
}

impl<B: Buf> Buf for ChunkLease<B> {
    fn remaining(&self) -> usize {
        self.content.remaining()
    }

    fn bytes(&self) -> &[u8] {
        self.content.bytes()
    }

    fn advance(&mut self, cnt: usize) {
        self.content.advance(cnt)
    }
}

unsafe impl<B: Buf> Send for ChunkLease<B> {}
// Drop automatically implemented
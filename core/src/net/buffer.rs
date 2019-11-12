use bytes::{BytesMut, BufMut, Buf};
use iovec::IoVec;
// use bytes::IntoBuf;
use crate::net::frames::*;
use crate::net::frames;
use std::collections::VecDeque;

pub const FRAME_HEAD_LEN: usize = frames::FRAME_HEAD_LEN as usize;
const INITIAL_BUFFER_LEN: usize = 10;
const MAX_POOL_SIZE: usize = 100;
const BUFFER_SIZE: usize = 655355;

pub struct BufferPool {
    pool: VecDeque<DecodeBuffer>,
    returned: VecDeque<DecodeBuffer>,
    pool_size: usize,
}

impl BufferPool {
    pub fn new() -> Self {
        let mut pool = VecDeque::<DecodeBuffer>::new();
        let mut returned= VecDeque::<DecodeBuffer>::new();
        for i in 0..INITIAL_BUFFER_LEN {
            // initialize 10 buffers
            pool.push_front(Self::new_buffer())
        }
        BufferPool{
            pool,
            returned,
            pool_size: INITIAL_BUFFER_LEN,
        }
    }

    fn new_buffer() -> DecodeBuffer {
        DecodeBuffer::new()
    }

    pub fn get_buffer(&mut self) -> Option<DecodeBuffer> {
        if let Some(new_buffer) = self.pool.pop_front() {
            return Some(new_buffer)
        }
        self.reclaim()
    }

    pub fn return_buffer(&mut self, buffer: DecodeBuffer) -> () {
        self.returned.push_back(buffer);
    }

    pub fn swap_buffer(&mut self, buffer: DecodeBuffer) -> Option<DecodeBuffer> {
        self.returned.push_back(buffer);
        if let Some(new_buffer) = self.pool.pop_front() {
            return Some(new_buffer)
        }
        self.reclaim()
    }

    fn reclaim(&mut self) -> Option<DecodeBuffer> {
        let len = self.returned.len();
        for i in 0..len {
            if let Some(mut returned_buffer) = self.returned.pop_front() {
                if returned_buffer.free() {
                    return Some(returned_buffer)
                } else {
                    self.returned.push_back(returned_buffer);
                }
            }
        }
        self.increase_pool()
    }

    fn increase_pool(&mut self) -> Option<DecodeBuffer> {
        if self.pool_size >= MAX_POOL_SIZE {
            return None
        };
        self.pool_size += 1;
        Some(Self::new_buffer())
    }
}

// Used to allow extraction of data frames in between inserting data
pub struct DecodeBuffer {
    buffer: BytesMut,
    next_frame_head: Option<FrameHead>,
}

impl DecodeBuffer{

    pub fn new() -> Self {
        DecodeBuffer{
            buffer: BytesMut::new(),
            next_frame_head: None,
        }
    }

    pub fn insert(&mut self, buffer: BytesMut) -> () {
        self.buffer.unsplit(buffer);
    }

    pub fn free(&mut self) -> bool {
        true
    }

    pub fn replace_buffer(&mut self, buffer: BytesMut) -> () {
        self.buffer = buffer;
    }

    pub fn get_frame(&mut self) -> Option<Frame> {
        // Check if we have head.
        //println!("Getting frame, current len {}", self.buffer.len());
        if let Some(head) = &self.next_frame_head {
            if self.buffer.len() >= head.content_length() {
                //println!("Decoding contents...");
                let mut data_buffer = self.buffer.split_to(head.content_length());
                let mut buf = data_buffer.freeze();
                match head.frame_type() {
                    FrameType::Data => {
                        if let Ok(data) = Data::decode_from(&mut buf) {
                            self.next_frame_head = None;
                            return Some(data);
                        } else {
                            //println!("Failied to decode data");
                        }
                    },
                    FrameType::StreamRequest => {
                        if let Ok(data) = StreamRequest::decode_from(&mut buf) {
                            self.next_frame_head = None;
                            return Some(data);
                        }
                    },
                    _ => {
                        println!("Weird head");
                        return None;
                    }
                }
            } else {
                //println!("unable to decode contents, buffer len: {}", self.buffer.len());
                return None;
            }
        } else {
            if self.buffer.len() >= FRAME_HEAD_LEN {
                let mut head_buffer = self.buffer.split_to(FRAME_HEAD_LEN);
                if let Ok(head) = FrameHead::decode_from(&mut head_buffer.freeze()) {
                    //println!("FrameHead decoded");
                    self.next_frame_head = Some(head);
                    return self.get_frame();
                } else {
                    // We are lost in the buffer, this is very bad
                    println!("Buffer split-off but not starting at a FrameHead");
                    return None;
                }
            } else {
                //println!("unable to decode head, buffer len: {}", self.buffer.len());
                return None;
            }
        }
        return None;
    }

    pub fn inner(&mut self) -> &mut BytesMut {
        return &mut self.buffer
    }
}
//! Frames are the core of the message transport layer, allowing applications to build
//! custom protocols atop this library.

use bytes::{Buf, BytesMut};
use bytes::BufMut;
use bytes::Bytes;
//use bytes::IntoBuf;
use std;
use std::fmt::Debug;
use std::sync::Arc;
use std::net::{SocketAddr, SocketAddrV4, Ipv4Addr, IpAddr, Ipv6Addr};
use crate::net::buffer::ChunkLease;
use crate::serialisation::serialisation_ids::{ACTOR_PATH, SYSTEM_PATH};
use crate::serialisation::{Deserialiser, SerError, SerId, Serialisable, Serialiser};
use crate::actors::SystemPath;
use crate::serialisation;
//use stream::StreamId;

pub const MAGIC_NUM: u32 = 0xC0A1BA11;
// (frame length) + (magic) + (frame type)
pub const FRAME_HEAD_LEN: u32 = 4 + 4 + 1;

#[derive(Debug)]
pub enum FramingError {
    BufferCapacity,
    UnsupportedFrameType,
    InvalidMagicNum,
    InvalidFrame,
    SerialisationError,
    OptionError,
    Io(std::io::Error),
}

impl From<std::io::Error> for FramingError {
    fn from(src: std::io::Error) -> Self {
        FramingError::Io(src)
    }
}

/// Core network frame definition
#[derive(Debug)]
pub enum Frame {
    StreamRequest(StreamRequest),
    CreditUpdate(CreditUpdate),
    Data(Data),
    Hello(Hello),
    Bye(),
}

impl Frame {
    pub fn frame_type(&self) -> FrameType {
        match *self {
            Frame::StreamRequest(_) => FrameType::StreamRequest,
            Frame::CreditUpdate(_) => FrameType::CreditUpdate,
            Frame::Data(_) => FrameType::Data,
            Frame::Hello(_) => FrameType::Hello,
            Frame::Bye() => FrameType::Bye,
        }
    }
    /*
    pub fn decode_from(buf: &mut ChunkLease<Bytes>) -> Result<Self, FramingError> {
        //let mut buf = buf.into_buf();
        let head = FrameHead::decode_from( buf)?;
        match head.frame_type {
            FrameType::StreamRequest => StreamRequest::decode_from( buf),
            FrameType::Data => Data::decode_from( buf),
            FrameType::CreditUpdate => CreditUpdate::decode_from( buf),
            _ => unimplemented!(),
        }
    }*/

    pub fn encode_into<B: BufMut>(&self, dst: &mut B) -> Result<(), ()> {
        let head = FrameHead::new(self.frame_type(), self.encoded_len());
        head.encode_into(dst, self.encoded_len() as u32);
        match *self {
            Frame::StreamRequest(ref frame) => frame.encode_into(dst),
            Frame::CreditUpdate(ref frame) => frame.encode_into(dst),
            Frame::Data(ref frame) => frame.encode_into(dst),
            Frame::Hello(ref frame) => frame.encode_into(dst),
            Frame::Bye() => Ok(()),
            _ => Err(()),
        }
    }

    /// Returns the number of bytes required to serialize this frame
    pub fn encoded_len(&self) -> usize {
        match *self {
            Frame::StreamRequest(ref frame) => frame.encoded_len(),
            Frame::CreditUpdate(ref frame) => frame.encoded_len(),
            Frame::Data(ref frame) => frame.encoded_len(),
            Frame::Hello(ref frame) => frame.encoded_len(),
            _ => 0,
        }
    }
}

pub trait FrameExt {
    fn decode_from(src: ChunkLease) -> Result<Frame, FramingError>;
    fn encode_into<B: BufMut>(&self, dst: &mut B) -> Result<(), ()>;
    fn encoded_len(&self) -> usize;
}

#[derive(Debug)]
pub struct StreamRequest {
    //pub stream_id: StreamId,
    pub credit_capacity: u32,
}

#[derive(Debug)]
pub struct CreditUpdate {
    //pub stream_id: StreamId,
    pub credit: u32,
}

#[derive(Debug)]
pub struct Data {
    pub payload: ChunkLease,
}

#[derive(Debug)]
pub struct Hello {
    //pub stream_id: StreamId,
    //pub seq_num: u32,
    pub addr: SocketAddr,
}

/// Byte-mappings for frame types
#[repr(u8)]
#[derive(Debug, Copy, Clone, PartialEq, Ord, PartialOrd, Eq)]
pub enum FrameType {
    StreamRequest = 0x01,
    Data = 0x02,
    CreditUpdate = 0x03,
    Hello = 0x04,
    Bye = 0x05,
    Unknown = 0x06,
}

impl From<u8> for FrameType {
    fn from(byte: u8) -> Self {
        match byte {
            0x01 => FrameType::StreamRequest,
            0x02 => FrameType::Data,
            0x03 => FrameType::CreditUpdate,
            0x04 => FrameType::Hello,
            0x05 => FrameType::Bye,
            _ => FrameType::Unknown,
        }
    }
}

// Head of each frame
#[derive(Debug)]
pub struct FrameHead {
    frame_type: FrameType,
    content_length: usize,
}

impl FrameHead {
    pub fn new(frame_type: FrameType, content_length: usize) -> Self {
        FrameHead { frame_type, content_length }
    }

    // Encodes own fields and entire frame length into `dst`.
    // This conforms to the length_delimited decoder found in the framed writer
    pub fn encode_into<B: BufMut>(&self, dst: &mut B, content_len: u32) {
        assert!(dst.remaining_mut() >= FRAME_HEAD_LEN as usize);
        // Represents total length, including bytes for encoding length
        // NOTE: This is not needed, and thus commented out, if length_delimited is also used for writing (as in the kompcis code)
        dst.put_u32(MAGIC_NUM);
        dst.put_u32(content_len);
        dst.put_u8(self.frame_type as u8);
    }

    pub fn decode_from<B: Buf>(src: &mut B) -> Result<Self, FramingError> {
        // length_delimited's decoder will have parsed the length out of `src`, subtract that out
        if src.remaining() < (FRAME_HEAD_LEN) as usize {
            return Err(FramingError::BufferCapacity);
        }

        let magic_check = src.get_u32();
        if magic_check != MAGIC_NUM {
            return Err(FramingError::InvalidMagicNum);
        }

        let content_length = src.get_u32() as usize;

        let frame_type: FrameType = src.get_u8().into();
        let head = FrameHead::new(frame_type, content_length);
        Ok(head)
    }

    pub fn content_length(&self) -> usize { self.content_length }
    pub fn frame_type(&self) -> FrameType {
        self.frame_type
    }

    pub fn encoded_len() -> usize {
        FRAME_HEAD_LEN as usize
    }
}

impl StreamRequest {
    pub fn new(credit_capacity: u32) -> Self {
        StreamRequest {
            credit_capacity,
        }
    }
}

impl Hello {
    pub fn new(addr: SocketAddr) -> Self {
        Hello {
            addr,
        }
    }
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }
}

impl Data {
    pub fn new(payload: ChunkLease) -> Self {
        Data {
            payload,
        }
    }

    /*
    pub fn with_raw_payload(raw_bytes: &[u8]) -> Self {
        Data::new(Bytes::from(raw_bytes))
    }
    */

    pub fn encoded_len(&self) -> usize {
        self.payload.bytes().len()
    }
/*
    pub fn payload_ref(&self) -> &Bytes {
        &self.payload
    }*/

    /// Consumes this frame and returns the raw payload buffer
    pub fn payload(self) -> ChunkLease {
        self.payload
    }
}

impl FrameExt for Data {
    fn decode_from(payload: ChunkLease) -> Result<Frame, FramingError> {
        /*if src.remaining() < 12 {
            return Err(FramingError::InvalidFrame);
        } */
        //let payload: Bytes = src.to_bytes();
        let data_frame = Data {
            payload,
        };
        Ok(Frame::Data(data_frame))
    }

    fn encode_into<B: BufMut>(&self, dst: &mut B) -> Result<(), ()> {
        // NOTE: This method _COPIES_ the owned bytes into `dst` rather than extending with the owned bytes
        let payload_len = self.payload.bytes().len();
        assert!(dst.remaining_mut() >= (self.encoded_len()));
        dst.put_slice(&self.payload.bytes());
        Ok(())
    }

    fn encoded_len(&self) -> usize {
        self.payload.bytes().len()
    }
}

impl FrameExt for StreamRequest {
    fn decode_from(mut src: ChunkLease) -> Result<Frame, FramingError> {
        if src.remaining() < 8 {
            return Err(FramingError::InvalidFrame);
        }
        //let stream_id: StreamId = src.get_u32_be().into();
        let credit = src.get_u32();
        let stream_req = StreamRequest {
            credit_capacity: credit,
        };
        Ok(Frame::StreamRequest(stream_req))
    }

    fn encode_into<B: BufMut>(&self, dst: &mut B) -> Result<(), ()> {
        assert!(dst.remaining_mut() >= self.encoded_len());
        //dst.put_u32_be(self.stream_id.into());
        dst.put_u32(self.credit_capacity);
        Ok(())
    }

    fn encoded_len(&self) -> usize {
        4 //stream_id + credit_capacity
    }
}

impl FrameExt for Hello {
    fn decode_from(mut src: ChunkLease) -> Result<Frame, FramingError> {
        match src.get_u8() {
            4 => {
                let ip = Ipv4Addr::from(src.get_u32());
                let port = src.get_u16();
                let addr = SocketAddr::new(IpAddr::V4(ip), port);
                Ok(Frame::Hello(Hello::new(addr)))
            }
            6 => {
                let ip = Ipv6Addr::from(src.get_u128());
                let port = src.get_u16();
                let addr = SocketAddr::new(IpAddr::V6(ip), port);
                Ok(Frame::Hello(Hello::new(addr)))
            }
            _ => {
                panic!("Faulty Hello Message!");
            }
        }
    }

    fn encode_into<B: BufMut>(&self, dst: &mut B) -> Result<(), ()> {
        match self.addr {
            SocketAddr::V4(v4) => {
                dst.put_u8(4); // version
                dst.put_slice(&v4.ip().octets()); // ip
                dst.put_u16(v4.port()); // port
                Ok(())
            }
            SocketAddr::V6(v6) => {
                dst.put_u8(6); // version
                dst.put_slice(&v6.ip().octets()); // ip
                dst.put_u16(v6.port()); // port
                Ok(())
            }
        }
    }

    fn encoded_len(&self) -> usize {
        match self.addr {
            SocketAddr::V4(v4) => {
                1 + 4 + 2 // version + ip + port
            }
            SocketAddr::V6(v6) => {
                1 + 16 + 2 // version + ip + port
            }
        }
    }
}

impl FrameExt for CreditUpdate {
    fn decode_from(mut src: ChunkLease) -> Result<Frame, FramingError> {
        unimplemented!()
    }

    fn encode_into<B: BufMut>(&self, dst: &mut B) -> Result<(), ()> {
        unimplemented!()
    }

    fn encoded_len(&self) -> usize {
        4 // stream_id + credit
    }
}

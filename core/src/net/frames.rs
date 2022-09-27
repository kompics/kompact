//! Frames are the core of the message transport layer, allowing applications to build
//! custom protocols atop this library.

use bytes::{Buf, BufMut};

//use bytes::IntoBuf;
use std::{self, fmt::Debug, io};

use crate::{net::buffers::ChunkLease, prelude::SessionId};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

//use stream::StreamId;
/// Used to identify start of a frame head
pub const MAGIC_NUM: u32 = 0xC0A1BA11;
// 192, 161, 186, 17
/// Framehead has constant size: (frame length) + (magic) + (frame type)
pub const FRAME_HEAD_LEN: u32 = 4 + 4 + 1;

/// Error messages for encoding/decoding
#[derive(Debug)]
pub enum FramingError {
    /// Encoding error, the buffer lacks capacity
    BufferCapacity,
    /// Uknown frame type.
    UnsupportedFrameType,
    /// Invalid start of frame.
    InvalidMagicNum((u32, Vec<u8>)),
    /// Invalid frame
    InvalidFrame,
    /// Serialisation during encode or deserialisation during decode
    SerialisationError,
    /// Unwrap error
    OptionError,
    /// No data to extract frame from
    NoData,
    /// IO errors wrapped into FramingError
    Io(std::io::Error),
}

impl From<std::io::Error> for FramingError {
    fn from(src: std::io::Error) -> Self {
        FramingError::Io(src)
    }
}

impl From<FramingError> for std::io::Error {
    fn from(_: FramingError) -> Self {
        io::Error::new(io::ErrorKind::InvalidData, "framing error")
    }
}

/// Core network frame definition
#[derive(Debug)]
pub enum Frame {
    /// Frame of Data
    Data(Data),
    /// Hello, used to initiate network channels
    Hello(Hello),
    /// Start, used to initiate network channels
    Start(Start),
    /// Ack to acknowledge that the connection is started.
    Ack(),
    /// Bye to signal that a channel is closing.
    Bye(),
}

impl Frame {
    /// Returns which FrameType a Frame is.
    pub fn frame_type(&self) -> FrameType {
        match *self {
            Frame::Data(_) => FrameType::Data,
            Frame::Hello(_) => FrameType::Hello,
            Frame::Start(_) => FrameType::Start,
            Frame::Ack() => FrameType::Ack,
            Frame::Bye() => FrameType::Bye,
        }
    }

    /// Encode a frame into a BufMut
    pub fn encode_into<B: BufMut>(&mut self, dst: &mut B) -> Result<(), FramingError> {
        let mut head = FrameHead::new(self.frame_type(), self.encoded_len());
        head.encode_into(dst);
        match self {
            Frame::Data(frame) => frame.encode_into(dst),
            Frame::Hello(frame) => frame.encode_into(dst),
            Frame::Start(frame) => frame.encode_into(dst),
            Frame::Ack() => Ok(()),
            Frame::Bye() => Ok(()),
        }
    }

    /// Returns the number of bytes required to serialize this frame
    pub fn encoded_len(&self) -> usize {
        match *self {
            Frame::Data(ref frame) => frame.encoded_len(),
            Frame::Hello(ref frame) => frame.encoded_len(),
            Frame::Start(ref frame) => frame.encoded_len(),
            _ => 0,
        }
    }
}

/// Trait for unifying the Encode/Decode of the different FrameTypes.
pub(crate) trait FrameExt {
    fn decode_from(src: ChunkLease) -> Result<Frame, FramingError>;
    fn encode_into<B: BufMut>(&mut self, dst: &mut B) -> Result<(), FramingError>;
    fn encoded_len(&self) -> usize;
}

/// Request Credits for credit-based flow-control
#[derive(Debug)]
pub struct StreamRequest {
    /// How many credits are requested
    pub credit_capacity: u32,
}

/// Give Credits for credit-based flow-control
#[derive(Debug)]
pub struct CreditUpdate {
    /// How many credits are given
    pub credit: u32,
}

/// Frame of Data
#[derive(Debug)]
pub struct Data {
    /// The contents of the Frame
    pub payload: ChunkLease,
}

/// Hello, used to initiate network channels
#[derive(Debug, Clone)]
pub struct Hello {
    /// The Cannonical Address of the host saying Hello
    pub addr: SocketAddr,
}

/// Hello, used to initiate network channels
#[derive(Debug)]
pub struct Start {
    /// The Cannonical Address of the host sending the Start message
    pub addr: SocketAddr,
    /// "Channel ID", used as a tie-breaker in mutual connection requests
    pub id: SessionId,
}

/// Byte-mappings for frame types
#[repr(u8)]
#[derive(Debug, Copy, Clone, PartialEq, Ord, PartialOrd, Eq)]
pub enum FrameType {
    /// Frame of Data
    Data = 0x02,
    /// Hello, used to initiate network channels
    Hello = 0x04,
    /// Start, used to initiate network channels
    Start = 0x05,
    /// Bye to signal that a channel is closing.
    Ack = 0x06,
    /// Bye to signal that a channel is closing.
    Bye = 0x07,
    /// Unknown frame type
    Unknown = 0x08,
}

impl From<u8> for FrameType {
    fn from(byte: u8) -> Self {
        match byte {
            0x02 => FrameType::Data,
            0x04 => FrameType::Hello,
            0x05 => FrameType::Start,
            0x06 => FrameType::Ack,
            0x07 => FrameType::Bye,
            _ => FrameType::Unknown,
        }
    }
}

/// Head of each frame
#[derive(Debug)]
pub(crate) struct FrameHead {
    frame_type: FrameType,
    content_length: usize,
}

impl FrameHead {
    pub(crate) fn new(frame_type: FrameType, content_length: usize) -> Self {
        FrameHead {
            frame_type,
            content_length,
        }
    }

    /// Encodes own fields and entire frame length into `dst`.
    /// This conforms to the length_delimited decoder found in the framed writer
    pub(crate) fn encode_into<B: BufMut>(&mut self, dst: &mut B) {
        assert!(dst.remaining_mut() >= FRAME_HEAD_LEN as usize);
        // Represents total length, including bytes for encoding length
        // NOTE: This is not needed, and thus commented out, if length_delimited is also used for writing (as in the kompcis code)
        dst.put_u32(MAGIC_NUM);
        dst.put_u32(self.content_length as u32);
        dst.put_u8(self.frame_type as u8);
    }

    pub(crate) fn decode_from<B: Buf + ?Sized>(src: &mut B) -> Result<Self, FramingError> {
        // length_delimited's decoder will have parsed the length out of `src`, subtract that out
        
        if src.remaining() < (FRAME_HEAD_LEN) as usize {
            return Err(FramingError::NoData);
        }
        let magic_check = src.get_u32();
        if magic_check != MAGIC_NUM {
            eprintln!("Magic check fail: {:X}", magic_check);
            return Err(FramingError::InvalidMagicNum((
                magic_check,
                src.chunk().to_vec(),
            )));
        }

        let content_length = src.get_u32() as usize;

        let frame_type: FrameType = src.get_u8().into();
        println!("frame type are {:?}", frame_type);

        let head = FrameHead::new(frame_type, content_length);
        println!("head are {:?}", head);

        Ok(head)
    }

    pub(crate) fn content_length(&self) -> usize {
        self.content_length
    }

    pub(crate) fn frame_type(&self) -> FrameType {
        self.frame_type
    }
}

impl Hello {
    /// Create a new hello message
    pub fn new(addr: SocketAddr) -> Self {
        Hello { addr }
    }

    /// Get the address sent in the Hello message
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }
}

impl Start {
    /// Create a new hello message
    pub fn new(addr: SocketAddr, id: SessionId) -> Self {
        Start { addr, id }
    }

    /// Get the address sent in the Start message
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Get the address sent in the Start message
    pub fn id(&self) -> SessionId {
        self.id
    }
}

impl Data {
    /// Create a new data frame
    pub fn new(payload: ChunkLease) -> Self {
        Data { payload }
    }

    pub(crate) fn encoded_len(&self) -> usize {
        self.payload.capacity()
    }

    /// Consumes this frame and returns the raw payload buffer
    pub(crate) fn payload(self) -> ChunkLease {
        self.payload
    }
}

impl FrameExt for Data {
    fn decode_from(payload: ChunkLease) -> Result<Frame, FramingError> {
        let data_frame = Data { payload };
        Ok(Frame::Data(data_frame))
    }

    /// This method should only be used for non-data frames, it copies and consumes the data!
    fn encode_into<B: BufMut>(&mut self, dst: &mut B) -> Result<(), FramingError> {
        // NOTE: This method _COPIES_ the owned bytes into `dst` rather than extending with the owned bytes
        assert!(dst.remaining_mut() >= (self.encoded_len()));
        while self.payload.has_remaining() {
            dst.put_slice(self.payload.chunk());
        }
        Ok(())
    }

    fn encoded_len(&self) -> usize {
        self.payload.chunk().len()
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

    fn encode_into<B: BufMut>(&mut self, dst: &mut B) -> Result<(), FramingError> {
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
            SocketAddr::V4(_v4) => {
                1 + 4 + 2 // version + ip + port
            }
            SocketAddr::V6(_v6) => {
                1 + 16 + 2 // version + ip + port
            }
        }
    }
}

impl FrameExt for Start {
    fn decode_from(mut src: ChunkLease) -> Result<Frame, FramingError> {
        match src.get_u8() {
            4 => {
                let ip = Ipv4Addr::from(src.get_u32());
                let port = src.get_u16();
                let addr = SocketAddr::new(IpAddr::V4(ip), port);
                let id = SessionId::from_u128(src.get_u128());
                Ok(Frame::Start(Start::new(addr, id)))
            }
            6 => {
                let ip = Ipv6Addr::from(src.get_u128());
                let port = src.get_u16();
                let addr = SocketAddr::new(IpAddr::V6(ip), port);
                let id = SessionId::from_u128(src.get_u128());
                Ok(Frame::Start(Start::new(addr, id)))
            }
            _ => {
                panic!("Faulty Hello Message!");
            }
        }
    }

    fn encode_into<B: BufMut>(&mut self, dst: &mut B) -> Result<(), FramingError> {
        match self.addr {
            SocketAddr::V4(v4) => {
                dst.put_u8(4); // version
                dst.put_slice(&v4.ip().octets()); // ip
                dst.put_u16(v4.port()); // port
                dst.put_u128(self.id.as_u128()); //id
                Ok(())
            }
            SocketAddr::V6(v6) => {
                dst.put_u8(6); // version
                dst.put_slice(&v6.ip().octets()); // ip
                dst.put_u16(v6.port()); // port
                dst.put_u128(self.id.as_u128()); //id
                Ok(())
            }
        }
    }

    fn encoded_len(&self) -> usize {
        match self.addr {
            SocketAddr::V4(_v4) => {
                1 + 4 + 2 + 16 // version + ip + port + uuid
            }
            SocketAddr::V6(_v6) => {
                1 + 16 + 2 + 16 // version + ip + port + uuid
            }
        }
    }
}

use super::*;
use crate::{
    messaging::SerialisedFrame,
    net::{
        buffer::{BufferChunk, DecodeBuffer},
        frames::{Ack, Frame, FramingError, Hello, Start, FRAME_HEAD_LEN},
    },
};
use bytes::{Buf, BytesMut};
use mio::{net::TcpStream, Token};
use network_thread::*;
use std::{
    cmp::Ordering,
    collections::VecDeque,
    fmt::Formatter,
    io,
    io::{Error, ErrorKind, Read, Write},
    net::{Shutdown::Both, SocketAddr},
};
use uuid::Uuid;

/// Received connection: Initialising -> Say Hello, Receive Start -> Connected, Send Ack
/// Requested connection: Requested -> Receive Hello -> Initialised -> Send Start, Receive Ack -> Connected
pub(crate) enum ChannelState {
    /// Requester state: outgoing request sent, await Hello(addr). SocketAddr is remote addr
    Requested(SocketAddr, Uuid),
    /// Receiver state: Hello(addr) must be sent on the channel, await Start(addr, Uuid)
    Initialising,
    /// Requester: Has received Hello(addr), must send Start(addr, Uuid) and await ack
    Initialised(SocketAddr, Uuid),
    /// The channel is ready to be used. Ack must be sent before anything else is sent
    Connected(SocketAddr, Uuid),
}

pub(crate) struct TcpChannel {
    stream: TcpStream,
    outbound_queue: VecDeque<SerialisedFrame>,
    pub token: Token,
    input_buffer: DecodeBuffer,
    pub state: ChannelState,
    pub messages: u32,
    own_addr: SocketAddr,
}

impl TcpChannel {
    pub fn new(
        stream: TcpStream,
        token: Token,
        buffer_chunk: BufferChunk,
        state: ChannelState,
        own_addr: SocketAddr,
    ) -> Self {
        let input_buffer = DecodeBuffer::new(buffer_chunk);
        TcpChannel {
            stream,
            outbound_queue: VecDeque::new(),
            token,
            input_buffer,
            state,
            messages: 0,
            own_addr,
        }
    }

    /// This is "network unsafe" to use. Please use the other interfaces for reading/writing.
    pub fn stream_mut(&mut self) -> &mut TcpStream {
        &mut self.stream
    }

    #[allow(dead_code)]
    pub fn stream(&self) -> &TcpStream {
        &self.stream
    }

    pub fn connected(&self) -> bool {
        matches!(self.state, ChannelState::Connected(_, _))
    }

    /// Internal helper function for special frames
    fn send_frame(&mut self, frame: Frame) -> () {
        let len = frame.encoded_len() + FRAME_HEAD_LEN as usize;
        let mut bytes = BytesMut::with_capacity(128);
        bytes.truncate(len);
        if let Ok(()) = frame.encode_into(&mut bytes) {
            self.outbound_queue
                .push_back(SerialisedFrame::Bytes(bytes.freeze()));
            // If there is a fatal error during a handshake the connection will be re-attempted
            let _ = self.try_drain();
        } else {
            panic!("Failed to encode bytes for Frame {:?}", frame.frame_type());
        }
    }

    pub fn get_id(&self) -> Option<Uuid> {
        match self.state {
            ChannelState::Connected(_, uuid) => Some(uuid),
            ChannelState::Requested(_, uuid) => Some(uuid),
            ChannelState::Initialised(_, uuid) => Some(uuid),
            _ => None,
        }
    }

    pub fn initialise(&mut self, addr: &SocketAddr) -> () {
        if let ChannelState::Initialising = self.state {
            // We must send enqueue Hello and await reply
            let hello = Frame::Hello(Hello::new(*addr));
            self.send_frame(hello);
        }
    }

    /// Must be called when a Hello frame is received on the channel.
    pub fn handle_hello(&mut self, hello: Hello) -> () {
        if let ChannelState::Requested(_, id) = self.state {
            // Has now received Hello(addr), must send Start(addr, uuid) and await ack
            let start = Frame::Start(Start::new(self.own_addr, id));
            self.send_frame(start);
            self.state = ChannelState::Initialised(hello.addr, id);
        }
    }

    /// Must be called when we Ack the channel. This means that the sender can start using the channel
    /// The receiver of the Ack must accept the Ack and use the channel.
    pub fn handle_start(&mut self, addr: &SocketAddr, id: Uuid) -> () {
        if let ChannelState::Initialising = self.state {
            // Method called because we received Start and want to send Ack.
            let ack = Frame::Ack(Ack { offset: 0 }); // we don't use offsets yet.
            self.send_frame(ack);
            self.state = ChannelState::Connected(*addr, id);
        }
    }

    /// Returns true if it transitioned, false if it's not starting.
    pub fn handle_ack(&mut self) -> bool {
        if let ChannelState::Initialised(addr, id) = self.state {
            // An Ack was received. Transition the channel.
            self.state = ChannelState::Connected(addr, id);
            true
        } else {
            eprintln!("Bad state reached during channel initialisation (handle_ack). Handshake went wrong.\
              Connection will likely fail and a re-connect will occur. Non fatal");
            false
        }
    }

    pub fn swap_buffer(&mut self, new_buffer: &mut BufferChunk) -> () {
        self.input_buffer.swap_buffer(new_buffer);
    }

    pub fn take_outbound(&mut self) -> Vec<SerialisedFrame> {
        let mut ret = Vec::new();
        while let Some(frame) = self.outbound_queue.pop_front() {
            ret.push(frame);
        }
        ret
    }

    /// This tries to read from the Tcp buffer into the DecodeBuffer, nothing else.
    pub fn receive(&mut self) -> io::Result<usize> {
        let mut read_bytes = 0;
        let mut sum_read_bytes = 0;
        let mut interrupts = 0;
        loop {
            // Keep all the read bytes in the buffer without overwriting
            if read_bytes > 0 {
                self.input_buffer.advance_writeable(read_bytes);
                read_bytes = 0;
            }
            if let Some(mut buf) = self.input_buffer.get_writeable() {
                match self.stream.read(&mut buf) {
                    Ok(0) => {
                        return Ok(sum_read_bytes);
                    }
                    Ok(n) => {
                        sum_read_bytes += n;
                        read_bytes = n;
                        // continue looping and reading
                    }
                    Err(err) if would_block(&err) => {
                        return Ok(sum_read_bytes);
                    }
                    Err(err) if interrupted(&err) => {
                        // We should continue trying until no interruption
                        interrupts += 1;
                        if interrupts >= network_thread::MAX_INTERRUPTS {
                            return Err(err);
                        }
                    }
                    Err(err) => {
                        return Err(err);
                    }
                }
            } else {
                return Err(Error::new(ErrorKind::InvalidData, "No space in Buffer"));
            }
        }
    }

    pub fn graceful_shutdown(&mut self) -> () {
        let bye = Frame::Bye();
        let mut bye_bytes = BytesMut::with_capacity(128);
        let len = bye.encoded_len() + FRAME_HEAD_LEN as usize;
        bye_bytes.truncate(len);
        //hello_bytes.extend_from_slice(&[0;hello.encoded_len()]);
        if let Ok(()) = bye.encode_into(&mut bye_bytes) {
            self.outbound_queue
                .push_back(SerialisedFrame::Bytes(bye_bytes.freeze()));
            let _ = self.try_drain(); // Try to drain outgoing
            let _ = self.receive(); // Try to drain incoming
        } else {
            panic!("Unable to send bye bytes, failed to encode!");
        }
        self.shutdown();
    }

    pub fn shutdown(&mut self) -> () {
        let _ = self.stream.shutdown(Both); // Discard errors while closing channels for now...
    }

    /// Tries to decode a frame from the DecodeBuffer.
    pub fn decode(&mut self) -> Result<Frame, FramingError> {
        match self.input_buffer.get_frame() {
            Ok(frame) => {
                self.messages += 1;
                Ok(frame)
            }
            Err(e) => Err(e),
        }
    }

    /// Enqueues the frame for sending on the channel.
    /// Enquing to a non-connected channel is disallowed.
    pub fn enqueue_serialised(&mut self, serialized: SerialisedFrame) -> () {
        self.outbound_queue.push_back(serialized);
    }

    /// Tries to drain the outbound buffer into
    pub fn try_drain(&mut self) -> io::Result<usize> {
        let mut sent_bytes: usize = 0;
        let mut interrupts = 0;
        while let Some(mut serialized_frame) = self.outbound_queue.pop_front() {
            match self.write_serialized(&serialized_frame) {
                Ok(n) => {
                    sent_bytes += n;
                    match &mut serialized_frame {
                        // Split the data and continue sending the rest later if we sent less than the full frame
                        SerialisedFrame::Bytes(bytes) => {
                            if n < bytes.len() {
                                //eprintln!("\n {:?} splitting bytes\n", self); // This shouldn't happen often.
                                let _ = bytes.split_to(n); // Discard the already sent split off part.
                                self.outbound_queue.push_front(serialized_frame);
                            }
                        }
                        SerialisedFrame::Chunk(chunk) => {
                            if n < chunk.bytes().len() {
                                //eprintln!("\n {:?} splitting bytes\n", self); // This shouldn't happen often.
                                chunk.advance(n);
                                self.outbound_queue.push_front(serialized_frame);
                            }
                        }
                    }
                    // Continue looping for the next message
                }
                // Would block "errors" are the OS's way of saying that the
                // connection is not actually ready to perform this I/O operation.
                Err(ref err) if would_block(err) => {
                    // re-insert the data at the front of the buffer and return
                    self.outbound_queue.push_front(serialized_frame);
                    return Ok(sent_bytes);
                }
                Err(err) if interrupted(&err) => {
                    // re-insert the data at the front of the buffer
                    self.outbound_queue.push_front(serialized_frame);
                    interrupts += 1;
                    if interrupts >= MAX_INTERRUPTS {
                        return Err(err);
                    }
                }
                // Other errors we'll consider fatal.
                Err(err) => {
                    self.outbound_queue.push_front(serialized_frame);
                    return Err(err);
                }
            }
        }
        Ok(sent_bytes)
    }

    /// No direct writing allowed, Must use other interface.
    fn write_serialized(&mut self, serialized: &SerialisedFrame) -> io::Result<usize> {
        match serialized {
            SerialisedFrame::Chunk(chunk) => self.stream.write(chunk.bytes()),
            SerialisedFrame::Bytes(bytes) => self.stream.write(bytes.bytes()),
        }
    }
}

impl std::fmt::Debug for TcpChannel {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TcpChannel")
            .field("State", &self.state)
            .field("Messages", &self.messages)
            .field("Read offset", &self.input_buffer.get_read_offset())
            .field("Write offset", &self.input_buffer.get_write_offset())
            .field("Outbound Queue", &self.outbound_queue.len())
            .finish()
    }
}

impl std::fmt::Debug for ChannelState {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ChannelState::Initialising => {
                f.debug_struct("ChannelState").field("State", &0).finish()
            }
            ChannelState::Initialised(addr, id) => f
                .debug_struct("ChannelState")
                .field("State", &0)
                .field("addr", addr)
                .field("id", id)
                .finish(),
            ChannelState::Requested(addr, id) => f
                .debug_struct("ChannelState")
                .field("State", &0)
                .field("addr", addr)
                .field("id", id)
                .finish(),
            ChannelState::Connected(addr, id) => f
                .debug_struct("ChannelState")
                .field("State", &1)
                .field("addr", addr)
                .field("id", id)
                .finish(),
        }
    }
}

#[derive(PartialEq, Eq)]
struct SocketWrapper {
    pub inner: SocketAddr,
}

impl std::cmp::PartialOrd for SocketWrapper {
    fn partial_cmp(&self, other: &SocketWrapper) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SocketWrapper {
    fn cmp(&self, other: &SocketWrapper) -> Ordering {
        match self.inner {
            SocketAddr::V4(_self_v4) => {
                if other.inner.is_ipv6() {
                    Ordering::Greater
                } else if self.inner.ip() == other.inner.ip() {
                    self.inner.port().cmp(&other.inner.port())
                } else {
                    self.inner.ip().cmp(&other.inner.ip())
                }
            }
            SocketAddr::V6(_self_v6) => {
                if other.inner.is_ipv4() {
                    Ordering::Greater
                } else if self.inner.ip() == other.inner.ip() {
                    self.inner.port().cmp(&other.inner.port())
                } else {
                    self.inner.ip().cmp(&other.inner.ip())
                }
            }
        }
    }
}

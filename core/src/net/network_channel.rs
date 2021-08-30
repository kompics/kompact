use super::*;
use crate::{
    messaging::SerialisedFrame,
    net::{
        buffers::{BufferChunk, BufferPool, DecodeBuffer},
        frames::{Frame, FramingError, Hello, Start, FRAME_HEAD_LEN},
    },
};
use bytes::{Buf, BytesMut};
use mio::{net::TcpStream, Token};
use network_thread::*;
use std::{
    cell::RefCell,
    cmp::Ordering,
    collections::VecDeque,
    fmt::Formatter,
    io,
    io::{Error, ErrorKind, Read, Write},
    net::{Shutdown::Both, SocketAddr},
};

/// Received connection: Initialising -> Say Hello, Receive Start -> Connected, Send Ack
/// Requested connection: Requested -> Receive Hello -> Initialised -> Send Start, Receive Ack -> Connected
pub(crate) enum ChannelState {
    /// Requester state: outgoing request sent, await Hello(addr). SocketAddr is remote addr
    Requested(SocketAddr, SessionId),
    /// Receiver state: Hello(addr) must be sent on the channel, await Start(addr, SessionId)
    Initialising,
    /// Requester: Has received Hello(addr), must send Start(addr, SessionId) and await ack
    Initialised(SocketAddr, SessionId),
    /// The channel is ready to be used. Ack must be sent before anything else is sent
    Connected(SocketAddr, SessionId),
    /// Local system initiated a graceful Channel Close, a Bye message must be sent and received
    CloseRequested(SocketAddr, SessionId),
    /// Remote system has initiated a graceful Channel Close, a Bye message must be sent
    CloseReceived(SocketAddr, SessionId),
    /// The channel is closing, will be dropped once the local `NetworkDispatcher` Acks the closing.
    Closed(SocketAddr, SessionId),
    /// There has been a fatal error on the Channel
    Error(Error),
}

impl ChannelState {
    fn session_id(&self) -> Option<SessionId> {
        match self {
            ChannelState::Requested(_, session) => Some(*session),
            ChannelState::Initialising => None,
            ChannelState::Initialised(_, session) => Some(*session),
            ChannelState::Connected(_, session) => Some(*session),
            ChannelState::CloseRequested(_, session) => Some(*session),
            ChannelState::CloseReceived(_, session) => Some(*session),
            ChannelState::Closed(_, session) => Some(*session),
            ChannelState::Error(_) => None,
        }
    }
}

pub(crate) struct TcpChannel {
    stream: TcpStream,
    outbound_queue: VecDeque<SerialisedFrame>,
    pub token: Token,
    address: SocketAddr,
    input_buffer: DecodeBuffer,
    pub state: ChannelState,
    pub messages: u32,
    own_addr: SocketAddr,
    nodelay: bool,
}

impl TcpChannel {
    pub fn new(
        stream: TcpStream,
        token: Token,
        address: SocketAddr,
        buffer_chunk: BufferChunk,
        state: ChannelState,
        own_addr: SocketAddr,
        network_config: &NetworkConfig,
    ) -> Self {
        let input_buffer = DecodeBuffer::new(buffer_chunk, network_config.get_buffer_config());
        TcpChannel {
            stream,
            outbound_queue: VecDeque::new(),
            token,
            address,
            input_buffer,
            state,
            messages: 0,
            own_addr,
            nodelay: network_config.get_tcp_nodelay(),
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

    pub fn session_id(&self) -> Option<SessionId> {
        self.state.session_id()
    }

    /// Internal helper function for special frames
    fn send_frame(&mut self, mut frame: Frame) -> () {
        let len = frame.encoded_len() + FRAME_HEAD_LEN as usize;
        let mut bytes = BytesMut::with_capacity(len);
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

    /// Checks the socket for errors and returns the current state
    pub fn read_state(&mut self) -> &ChannelState {
        match self.stream.take_error() {
            Ok(Some(error)) => {
                self.state = ChannelState::Error(error);
            }
            Err(error) => {
                self.state = ChannelState::Error(error);
            }
            _ => (), // ignore
        }
        &self.state
    }

    pub fn initialise(&mut self, addr: &SocketAddr) -> () {
        if let ChannelState::Initialising = self.state {
            // We must send enqueue Hello and await reply
            let hello = Frame::Hello(Hello::new(*addr));
            self.send_frame(hello);
        }
    }

    /// Must be called when a Hello frame is received on the channel.
    pub fn handle_hello(&mut self, hello: &Hello) -> () {
        if let ChannelState::Requested(_, id) = self.state {
            // Has now received Hello(addr), must send Start(addr, SessionId) and await ack
            let start = Frame::Start(Start::new(self.own_addr, id));
            self.send_frame(start);
            self.state = ChannelState::Initialised(hello.addr, id);
            self.address = hello.addr;
        }
    }

    /// Must be called when we Ack the channel. This means that the sender can start using the channel
    /// The receiver of the Ack must accept the Ack and use the channel.
    pub fn handle_start(&mut self, start: &Start) -> () {
        if let ChannelState::Initialising = self.state {
            // Method called because we received Start and want to send Ack.
            let ack = Frame::Ack();
            self.stream
                .set_nodelay(self.nodelay)
                .expect("set nodelay failed");
            self.send_frame(ack);
            self.state = ChannelState::Connected(start.addr, start.id);
            self.address = start.addr;
        }
    }

    pub fn address(&self) -> SocketAddr {
        self.address
    }

    pub fn handle_ack(&mut self) -> () {
        if let ChannelState::Initialised(addr, id) = self.state {
            // An Ack was received. Transition the channel.
            self.stream
                .set_nodelay(self.nodelay)
                .expect("set nodelay failed");
            self.state = ChannelState::Connected(addr, id);
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

    /// Performs receive and decode, should be called repeatedly
    /// May return `Ok(Frame::Data)`, `Ok(Frame::Start)`, `Ok(Frame::Bye)`, or an Error.
    pub fn read_frame(&mut self, buffer_pool: &RefCell<BufferPool>) -> io::Result<Option<Frame>> {
        if !self.input_buffer.has_frame()? {
            match self.receive() {
                Ok(_) => {}
                Err(err) if no_buffer_space(&err) => {
                    if !&self.input_buffer.has_frame()? {
                        let mut pool = buffer_pool.borrow_mut();
                        let mut buffer_chunk = pool.get_buffer().ok_or(err)?;
                        self.swap_buffer(&mut buffer_chunk);
                        pool.return_buffer(buffer_chunk);
                        drop(pool);
                        return self.read_frame(buffer_pool);
                    }
                }
                Err(err) => {
                    return Err(err);
                }
            };
        }
        match self.decode() {
            Ok(Frame::Hello(hello)) => Ok(Some(Frame::Hello(hello))),
            Ok(Frame::Ack()) => {
                self.handle_ack();
                Ok(Some(Frame::Ack()))
            }
            Ok(Frame::Bye()) => {
                self.handle_bye();
                Ok(Some(Frame::Bye()))
            }
            Ok(frame) => Ok(Some(frame)),
            Err(FramingError::NoData) => Ok(None),
            Err(_) => Err(Error::new(ErrorKind::InvalidData, "Framing Error")),
        }
    }

    /// This tries to read from the Tcp buffer into the DecodeBuffer, nothing else.
    fn receive(&mut self) -> io::Result<()> {
        let mut interrupts = 0;
        loop {
            let mut buf = self
                .input_buffer
                .get_writeable()
                .ok_or_else(|| io::Error::new(ErrorKind::InvalidInput, "No Buffer Space"))?;
            match self.stream.read(&mut buf) {
                Ok(0) => {
                    return Ok(());
                }
                Ok(n) => {
                    self.input_buffer.advance_writeable(n);
                }
                Err(err) if would_block(&err) => {
                    return Ok(());
                }
                Err(err) if interrupted(&err) => {
                    interrupts += 1;
                    if interrupts >= network_thread::MAX_INTERRUPTS {
                        return Err(err);
                    }
                }
                Err(err) => {
                    return Err(err);
                }
            }
        }
    }

    /// Enqueues a Bye message, and attempts to drain the message, returning Ok if the bye was sent
    pub fn send_bye(&mut self) -> io::Result<()> {
        let mut bye = Frame::Bye();
        let mut bye_bytes = BytesMut::with_capacity(128);
        let len = bye.encoded_len() + FRAME_HEAD_LEN as usize;
        bye_bytes.truncate(len);
        if let Ok(()) = bye.encode_into(&mut bye_bytes) {
            self.outbound_queue
                .push_back(SerialisedFrame::Bytes(bye_bytes.freeze()));
            let _ = self.try_drain(); // Try to drain outgoing
        } else {
            panic!("Unable to send bye bytes, failed to encode!");
        }
        if self.outbound_queue.is_empty() {
            io::Result::Ok(())
        } else {
            // Need to wait for the message to be sent again
            io::Result::Err(Error::new(ErrorKind::Interrupted, "bye not sent"))
        }
    }

    /// Handles a Bye message. If the method returns Ok it is safe to shutdown.
    pub fn handle_bye(&mut self) -> () {
        match self.state {
            ChannelState::Connected(addr, id) => {
                self.state = ChannelState::CloseReceived(addr, id);
                if self.send_bye().is_ok() {
                    self.state = ChannelState::Closed(addr, id);
                }
            }
            ChannelState::CloseRequested(addr, id) => self.state = ChannelState::Closed(addr, id),
            _ => {}
        }
    }

    /// If the channel is in connected the channel transitions to CloseRequested
    /// and sends a bye message
    pub fn initiate_graceful_shutdown(&mut self) -> () {
        if let ChannelState::Connected(addr, id) = self.state {
            self.state = ChannelState::CloseRequested(addr, id);
            let _ = self.send_bye();
        }
    }

    /// Shuts down the channel stream
    pub fn shutdown(&mut self) -> () {
        let _ = self.stream.shutdown(Both); // Discard errors while closing channels for now...
        if let ChannelState::Connected(addr, id) = self.state {
            self.state = ChannelState::Closed(addr, id);
        }
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
                                let _ = bytes.split_to(n); // Discard the already sent split off part.
                                self.outbound_queue.push_front(serialized_frame);
                            }
                        }
                        SerialisedFrame::ChunkLease(chunk) => {
                            if n < chunk.remaining() {
                                chunk.advance(n);
                                self.outbound_queue.push_front(serialized_frame);
                            }
                        }
                        SerialisedFrame::ChunkRef(chunk) => {
                            if n < chunk.remaining() {
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
                    self.outbound_queue.push_front(serialized_frame);
                    return Ok(sent_bytes);
                }
                Err(err) if interrupted(&err) => {
                    self.outbound_queue.push_front(serialized_frame);
                    interrupts += 1;
                    if interrupts >= MAX_INTERRUPTS {
                        return Err(err);
                    }
                }
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
            SerialisedFrame::ChunkLease(chunk) => self.stream.write(chunk.chunk()),
            SerialisedFrame::Bytes(bytes) => self.stream.write(bytes.chunk()),
            SerialisedFrame::ChunkRef(chunkref) => self.stream.write(chunkref.chunk()),
        }
    }

    pub(crate) fn kill(&mut self) -> () {
        let _ = self.stream.shutdown(Both);
    }
}

impl std::fmt::Debug for TcpChannel {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TcpChannel")
            .field("State", &self.state)
            .field("Messages", &self.messages)
            .field("Decode Buffer", &self.input_buffer)
            .field("Outbound Queue", &self.outbound_queue.len())
            .finish()
    }
}

impl std::fmt::Debug for ChannelState {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let mut ds = f.debug_struct("ChannelState");
        ds.field("State", &0);
        match self {
            ChannelState::Initialised(addr, id) => ds.field("Address", addr).field("Id", id),
            ChannelState::Requested(addr, id) => ds.field("Address", addr).field("Id", id),
            ChannelState::Connected(addr, id) => ds.field("Address", addr).field("Id", id),
            ChannelState::Closed(addr, id) => ds.field("Address", addr).field("Id", id),
            ChannelState::CloseRequested(addr, id) => ds.field("Address", addr).field("Id", id),
            ChannelState::CloseReceived(addr, id) => ds.field("Address", addr).field("Id", id),
            ChannelState::Error(error) => ds.field("Error", error),
            ChannelState::Initialising => &mut ds,
        }
        .finish()
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

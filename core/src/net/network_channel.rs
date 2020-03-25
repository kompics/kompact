use crate::{
    messaging::SerializedFrame,
    net::{
        buffer::{BufferChunk, DecodeBuffer},
        frames::{Frame, Hello},
        network_thread,
        network_thread::*,
        ConnectionState,
    },
};
use bytes::{Buf, BytesMut};
use core::mem;
use mio::{net::TcpStream, Interest, Token};
use std::{
    cmp::Ordering,
    collections::VecDeque,
    io,
    io::{Error, ErrorKind, Write, Read},
    net::{Shutdown::Both, SocketAddr},
};

pub(crate) struct TcpChannel {
    stream: TcpStream,
    outbound_queue: VecDeque<SerializedFrame>,
    pub token: Token,
    input_buffer: DecodeBuffer,
    pub state: ConnectionState,
    pub messages: u32,
}

impl TcpChannel {
    pub fn new(stream: TcpStream, token: Token, buffer_chunk: BufferChunk) -> Self {
        let input_buffer = DecodeBuffer::new(buffer_chunk);
        TcpChannel {
            stream,
            outbound_queue: VecDeque::new(),
            token,
            input_buffer,
            state: ConnectionState::New,
            messages: 0,
        }
    }

    pub fn stream_mut(&mut self) -> &mut TcpStream {
        &mut self.stream
    }

    pub fn stream(&self) -> &TcpStream {
        &self.stream
    }

    pub fn has_remaining_output(&self) -> bool {
        if self.outbound_queue.len() > 0 {
            true
        } else {
            false
        }
    }

    pub fn initialize(&mut self, addr: &SocketAddr) -> io::Result<()> {
        let hello = Frame::Hello(Hello::new(*addr));
        let mut hello_bytes = BytesMut::with_capacity(hello.encoded_len());
        //hello_bytes.extend_from_slice(&[0;hello.encoded_len()]);
        if let Ok(()) = hello.encode_into(&mut hello_bytes) {
            if let Err(e) = self.stream.set_nodelay(true) {
                eprintln!("failed to set nodelay on TcpChannel connected to {:?}: {:?}", self.stream.peer_addr(), e);
            }
            self.outbound_queue
                .push_back(SerializedFrame::Bytes(hello_bytes.freeze()));
            self.try_drain()?;
        } else {
            panic!("Unable to send hello bytes, failed to encode!");
        }
        Ok(())
    }

    pub fn swap_buffer(&mut self, new_buffer: &mut BufferChunk) -> () {
        self.input_buffer.swap_buffer(new_buffer);
    }

    pub fn receive(&mut self) -> io::Result<usize> {
        let mut read_bytes = 0;
        let mut sum_read_bytes = 0;
        let mut interrupts = 0;
        loop {
            // Keep all the read bytes in the buffer without overwriting
            if read_bytes > 0 {
                self.input_buffer.advance_writeable(read_bytes);
            }
            if let Some(mut buf) = self.input_buffer.get_writeable() {
                match self.stream.read(&mut buf) {
                    Ok(0) => {
                        return Ok(sum_read_bytes);
                    }
                    Ok(n) => {
                        sum_read_bytes = sum_read_bytes + n;
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
                        //println!("network thread {} got uncaught error during read from remote {}", self.stream.local_addr().unwrap(), self.stream.peer_addr().unwrap());
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
        let mut bye_bytes = BytesMut::with_capacity(bye.encoded_len());
        //hello_bytes.extend_from_slice(&[0;hello.encoded_len()]);
        if let Ok(()) = bye.encode_into(&mut bye_bytes) {
            self.outbound_queue
                .push_back(SerializedFrame::Bytes(bye_bytes.freeze()));
            let _ = self.try_drain(); // Try to drain outgoing
            let _ = self.receive(); // Try to drain incoming
        } else {
            panic!("Unable to send bye bytes, failed to encode!");
        }
        self.shutdown();
    }

    pub fn shutdown(&mut self) -> () {
        //self.receive();
        let _ = self.stream.shutdown(Both); // Discard errors while closing channels for now...
        self.state = ConnectionState::Closed;
    }

    pub fn decode(&mut self) -> Option<Frame> {
        if let Some(frame) = self.input_buffer.get_frame() {
            self.messages += 1;
            return Some(frame);
        }
        None
    }

    pub fn enqueue_serialized(&mut self, serialized: SerializedFrame) -> () {
        self.outbound_queue.push_back(serialized);
    }

    pub fn try_drain(&mut self) -> io::Result<usize> {
        let mut sent_bytes: usize = 0;
        let mut interrupts = 0;
        while let Some(mut serialized_frame) = self.outbound_queue.pop_front() {
            match self.write_serialized(&serialized_frame) {
                Ok(n) => {
                    sent_bytes = sent_bytes + n;
                    match &mut serialized_frame {
                        // Split the data and continue sending the rest later if we sent less than the full frame
                        SerializedFrame::Bytes(bytes) => {
                            if n < bytes.len() {
                                let _ = bytes.split_to(n); // Discard the already sent split off part.
                                self.outbound_queue.push_front(serialized_frame);
                            }
                        }
                        SerializedFrame::Chunk(chunk) => {
                            if n < chunk.bytes().len() {
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

    pub fn write_serialized(&mut self, serialized: &SerializedFrame) -> io::Result<usize> {
        match serialized {
            SerializedFrame::Chunk(chunk) => self.stream.write(chunk.bytes()),
            SerializedFrame::Bytes(bytes) => self.stream.write(bytes.bytes()),
        }
    }

    /// Compares the SocketAddr the two channels are bound on and ensures that both sides will merge into the same channel on both sides
    /// The channel to retain will be self while the unused other will be stopped.
    pub fn merge(&mut self, other: &mut TcpChannel) -> () {
        let this_peer = SocketWrapper {
            inner: self.stream.peer_addr().unwrap(),
        };
        let that_peer = SocketWrapper {
            inner: other.stream.peer_addr().unwrap(),
        };
        let this_local = SocketWrapper {
            inner: self.stream.local_addr().unwrap(),
        };
        let that_local = SocketWrapper {
            inner: other.stream.local_addr().unwrap(),
        };

        if (this_peer < that_peer && this_peer < this_local && this_peer < that_local)
            || (this_local < that_local && this_local < this_peer && this_local < that_local)
        {
            // Keep self, this_local or this_peer was the lowest in the set
        } else {
            // Keep other
            mem::swap(other.stream_mut(), self.stream_mut());
        }
        other.shutdown();
        if other.outbound_queue.len() > 0 {
            self.outbound_queue.append(&mut other.outbound_queue);
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
                } else {
                    if self.inner.ip() == other.inner.ip() {
                        self.inner.port().cmp(&other.inner.port())
                    } else {
                        self.inner.ip().cmp(&other.inner.ip())
                    }
                }
            }
            SocketAddr::V6(_self_v6) => {
                if other.inner.is_ipv4() {
                    Ordering::Greater
                } else {
                    if self.inner.ip() == other.inner.ip() {
                        self.inner.port().cmp(&other.inner.port())
                    } else {
                        self.inner.ip().cmp(&other.inner.ip())
                    }
                }
            }
        }
    }
}

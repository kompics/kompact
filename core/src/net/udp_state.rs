use super::*;
use crate::{
    messaging::{NetMessage, SerialisedFrame},
    net::buffer::{BufferChunk, DecodeBuffer},
};
use mio::net::UdpSocket;
use network_thread::*;
use std::{collections::VecDeque, io, net::SocketAddr};

const MAX_PACKET_SIZE: usize = 65536;

pub(super) struct UdpState {
    logger: KompactLogger,
    socket: UdpSocket,
    outbound_queue: VecDeque<(SocketAddr, SerialisedFrame)>,
    input_buffer: DecodeBuffer,
    pub(super) incoming_messages: VecDeque<NetMessage>,
}

impl UdpState {
    pub(super) fn new(socket: UdpSocket, buffer_chunk: BufferChunk, logger: KompactLogger) -> Self {
        UdpState {
            logger,
            socket,
            outbound_queue: VecDeque::new(),
            input_buffer: DecodeBuffer::new(buffer_chunk),
            incoming_messages: VecDeque::new(),
        }
    }

    pub(super) fn try_write(&mut self) -> io::Result<usize> {
        let mut sent_bytes: usize = 0;
        let mut interrupts = 0;
        while let Some((addr, frame)) = self.outbound_queue.pop_front() {
            if frame.len() > MAX_PACKET_SIZE {
                warn!(
                    self.logger,
                    "A UDP package to {} was too large and has been dropped: {} > {}",
                    addr,
                    frame.len(),
                    MAX_PACKET_SIZE
                );
            } else {
                match self.socket.send_to(frame.bytes(), addr) {
                    Ok(n) => {
                        // This really shouldn't happen, and can lead to inconsistent network messages
                        assert_eq!(n, frame.len(), "A UDP frame was written incompletely!");
                        sent_bytes += n;
                    }
                    Err(ref err) if would_block(err) => {
                        // re-insert the data at the front of the buffer and return
                        self.outbound_queue.push_front((addr, frame));
                        return Ok(sent_bytes);
                    }
                    Err(err) if interrupted(&err) => {
                        // re-insert the data at the front of the buffer
                        self.outbound_queue.push_front((addr, frame));
                        interrupts += 1;
                        if interrupts >= MAX_INTERRUPTS {
                            return Err(err);
                        }
                    }
                    // Other errors we'll consider fatal.
                    Err(err) => {
                        self.outbound_queue.push_front((addr, frame));
                        return Err(err);
                    }
                }
            }
        }
        Ok(sent_bytes)
    }

    pub(super) fn try_read(&mut self) -> io::Result<(usize, IOReturn)> {
        let mut received_bytes: usize = 0;
        let mut interrupts = 0;
        loop {
            if let Some(mut buf) = self.input_buffer.get_writeable() {
                if buf.len() < MAX_PACKET_SIZE {
                    debug!(
                        self.logger,
                        "Swapping UDP buffer as only {} bytes remain.",
                        buf.len()
                    );
                    return Ok((received_bytes, IOReturn::SwapBuffer));
                }
                match self.socket.recv_from(&mut buf) {
                    Ok((0, addr)) => {
                        debug!(self.logger, "Got empty UDP datagram from {}", addr);
                        return Ok((received_bytes, IOReturn::None));
                    }
                    Ok((n, addr)) => {
                        received_bytes += n;
                        self.input_buffer.advance_writeable(n);
                        self.decode_message(addr);
                    }
                    Err(err) if would_block(&err) => {
                        return Ok((received_bytes, IOReturn::None));
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
                return Ok((received_bytes, IOReturn::SwapBuffer));
            }
        }
    }

    fn decode_message(&mut self, source: SocketAddr) {
        match self.input_buffer.get_frame() {
            Ok(Frame::Data(frame)) => {
                use serialisation::ser_helpers::deserialise_msg;
                let buf = frame.payload();
                match deserialise_msg(buf) {
                    Ok(envelope) => self.incoming_messages.push_back(envelope),
                    Err(e) => {
                        warn!(
                            self.logger,
                            "Could not deserialise UDP frame from {}: {}", source, e
                        );
                    }
                }
            }
            Ok(frame) => {
                warn!(
                    self.logger,
                    "Decoded unexpected frame from UDP datagram from {}: {:?}", source, frame
                );
            }
            Err(e) => {
                warn!(
                    self.logger,
                    "Could not decode UDP datagram from {}: {:?}", source, e
                );
            }
        }
    }

    pub(super) fn enqueue_serialised(&mut self, addr: SocketAddr, frame: SerialisedFrame) -> () {
        self.outbound_queue.push_back((addr, frame));
    }

    pub(super) fn swap_buffer(&mut self, new_buffer: &mut BufferChunk) -> () {
        self.input_buffer.swap_buffer(new_buffer);
    }
}

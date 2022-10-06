use super::*;
use crate::{
    messaging::{NetMessage, SerialisedFrame},
    net::buffers::{BufferChunk, BufferPool, DecodeBuffer, decode_buffer},
};
use mio::net::UdpSocket;
use network_thread::*;
use quinn_proto::Transmit;
use std::{cell::RefCell, cmp::min, collections::VecDeque, io, io::Error, net::SocketAddr};

// Note that this is a theoretical IPv4 limit.
// This may be violated with IPv6 jumbograms.
// More importantly, individual OSs can have their limit much lower!
const MAX_PACKET_SIZE: usize = 65535;

pub(super) struct UdpState {
    logger: KompactLogger,
    pub(super) socket: UdpSocket,
    pub(super) outbound_queue: VecDeque<(SocketAddr, SerialisedFrame)>,
    pub(super) input_buffer: DecodeBuffer,
    pub(super) incoming_messages: VecDeque<NetMessage>,
    max_packet_size: usize,
}

impl UdpState {
    pub(super) fn new(
        socket: UdpSocket,
        buffer_chunk: BufferChunk,
        logger: KompactLogger,
        network_config: &NetworkConfig,
    ) -> Self {
        // If chunk_size is smaller than MAX_PACKET_SIZE we will use that size as the limit instead.
        let chunk_size = network_config.get_buffer_config().chunk_size;
        let max_packet_size = min(chunk_size, MAX_PACKET_SIZE);
        UdpState {
            logger,
            socket,
            outbound_queue: VecDeque::new(),
            input_buffer: DecodeBuffer::new(buffer_chunk, network_config.get_buffer_config()),
            incoming_messages: VecDeque::new(),
            max_packet_size,
        }
    }

    pub(super) fn pending_messages(&self) -> usize {
        self.outbound_queue.len()
    }

    pub(super) fn try_write(&mut self) -> io::Result<usize> {
        let mut sent_bytes: usize = 0;
        let mut interrupts = 0;
        while let Some((addr, mut frame)) = self.outbound_queue.pop_front() {
            frame.make_contiguous();
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
        Ok(sent_bytes)
    }

    fn swap_buffer(&mut self, buffer_pool: &RefCell<BufferPool>) -> () {
        let mut pool = buffer_pool.borrow_mut();
        if let Some(mut buffer) = pool.get_buffer() {
            debug!(
                self.logger,
                "Swapping UDP buffer as only {} bytes remain.",
                self.input_buffer.writeable_len()
            );
            self.input_buffer.swap_buffer(&mut buffer);
            pool.return_buffer(buffer);
        }
    }

    pub(super) fn try_read(&mut self, buffer_pool: &RefCell<BufferPool>) -> io::Result<usize> {
        let mut interrupts = 0;
        loop {
            if self.input_buffer.writeable_len() < self.max_packet_size {
                self.swap_buffer(buffer_pool);
                if self.input_buffer.writeable_len() < self.max_packet_size {
                    return Err(Error::new(io::ErrorKind::InvalidInput, "Out of Buffers"));
                }
            }
            if let  Some(buf) = self.input_buffer.get_writeable() {
                match self.socket.recv_from(buf) {
                    Ok((0, addr)) => {
                        debug!(self.logger, "Got empty UDP datagram from {}", addr);
                        return Ok(0);
                    }
                    Ok((n, addr)) => {
                        println!("n received {:?}", n);
                       // self.input_buffer.read_chunk_lease(n);
                        self.input_buffer.advance_writeable(n);
                        //self.decode_message(addr);
                        return Ok(n);
                    }
                    Err(err) if would_block(&err) => {
                        //return Ok((_));
                        return Err(err);

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
                return Err(Error::new(io::ErrorKind::InvalidInput, "Out of Buffers"));
            }
        }
    }

    fn decode_message(&mut self, source: SocketAddr) {
       // println!("print input buffer get frame {:?}", self.input_buffer.get_frame());
        match self.input_buffer.get_frame() {
            Ok(Frame::Data(frame)) => {
                use serialisation::ser_helpers::deserialise_chunk_lease;
                let buf = frame.payload();
                match deserialise_chunk_lease(buf) {
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
            Err(FramingError::InvalidMagicNum(message)) => {
                let len = message.1.len();
                let chunklease = DecodeBuffer::read_chunk_lease(&mut self.input_buffer, len);
                println!("chunklease {:?}", chunklease);

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
}

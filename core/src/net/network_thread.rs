use super::*;
use crate::net::NetworkEvent;
use std::sync::mpsc::{channel, Receiver as Recv, Sender};
use std::{collections::{HashMap, VecDeque, LinkedList}, net::SocketAddr, usize, vec::Vec, io::{self, Read, Write, Error, ErrorKind}, mem};
use mio::{{Events, Poll, Ready, PollOpt, Token, Registration}, event::Event, net::{TcpListener, TcpStream}, Evented};
use crate::net::{
    frames::*,
    buffer::DecodeBuffer,
    events::*,
    ConnectionState,
};
use crossbeam_queue::SegQueue;
use iovec::IoVec;
use std::borrow::Borrow;
use bytes::{BytesMut, BufMut, Buf};
use std::ops::Deref;
use std::borrow::BorrowMut;
use std::net::Shutdown::Both;


/*
    Using https://github.com/tokio-rs/mio/blob/master/examples/tcp_server.rs as template.
    Single threaded MIO event loop.
    Receives outgoing Box<IoVec> via SegQueue, notified by polling via the "Registration"
    Buffers the outgoing Boxes for different connections in a hashmap.
    Will Send incoming messages directly to components.
*/

// Used for identifying connections
const SERVER: Token = Token(0);
const START_TOKEN: Token = Token(1);
// Used for identifying the dispatcher/input queue
const DISPATCHER: Token = Token(usize::MAX-1);
const READ_BUFFER_SIZE: usize = 655355;
const MAX_POLL_EVENTS: usize = 1024;

pub struct NetworkThread {
    //log: KompactLogger,
    pub addr: SocketAddr,
    //connection_events: UnboundedSender<NetworkEvent>,
    lookup: Arc<ArcSwap<ActorStore>>,
    listener: Box<TcpListener>,
    poll: Poll,
    // Contains K,V=Remote SocketAddr, Output buffer; Token for polling; Input-buffer,
    stream_map: HashMap<SocketAddr, (TcpStream, VecDeque<Box<BytesMut>>, Token, Box<BytesMut>, Box<DecodeBuffer>)>,
    token_map: HashMap<Token, SocketAddr>,
    token: Token,
    input_queue: Box<Recv<DispatchEvent>>,
    dispatcher_registration: Registration,
    dispatcher_set_readiness: SetReadiness,
    sent_bytes: u64,
    received_bytes: u64,
    sent_msgs: u64,
    received_msgs: u64,
    stopped: bool,
    network_thread_sender: Sender<bool>,
}

impl NetworkThread {
    pub fn new(
        //log: KompactLogger,
        addr: SocketAddr,
        //connection_events: UnboundedSender<NetworkEvent>,
        lookup: Arc<ArcSwap<ActorStore>>,
        dispatcher_registration: Registration,
        dispatcher_set_readiness: SetReadiness,
        input_queue: Recv<DispatchEvent>,
        network_thread_sender: Sender<bool>,
    ) -> NetworkThread {
        // Set-up the Listener
        if let Ok(listener) = TcpListener::bind(&addr) {
            let actual_addr = listener
                .local_addr()
                .expect("could not get real addr");
            // Set up polling for the Dispatcher and the listener.
            let mut poll = Poll::new()
                .expect("failed to create Poll instance in NetworkThread");
            poll.register(&listener, SERVER, Ready::readable(), PollOpt::edge() | PollOpt::oneshot())
                .expect("failed to register TCP SERVER");
            poll.register(&dispatcher_registration, DISPATCHER, Ready::readable(), PollOpt::edge() | PollOpt::oneshot())
                .expect("failed to register dispatcher Poll");
            // Connections and buffers
            // There's probably a better way to handle the token/addr maps
            let mut stream_map = HashMap::new();
            let mut token_map = HashMap::new();

            NetworkThread {
                //log,
                addr: actual_addr,
                //connection_events,
                lookup,
                listener: Box::new(listener),
                poll,
                stream_map,
                token_map,
                token: START_TOKEN,
                input_queue: Box::new(input_queue),
                dispatcher_registration,
                dispatcher_set_readiness,
                sent_bytes: 0,
                received_bytes: 0,
                sent_msgs: 0,
                received_msgs: 0,
                stopped: false,
                network_thread_sender,
            }
        } else {
            panic!("NetworkThread failed to bind to address!");
        }

    }

    // Event loop
    pub fn run(&mut self) -> () {
        let mut events = Events::with_capacity(MAX_POLL_EVENTS);
        //println!("network thread running");
        loop {
            //println!("network thread {} polling with current received msgs {} and sent msgs {}", self.addr, self.received_msgs, self.sent_msgs);
            self.poll.poll(&mut events, None)
                .expect("Error when calling Poll");
            // time-out can be used for work stealing
            for event in events.iter() {
                //println!("network thread {} handling event {}, readable {} writable {}", self.addr, event.token().0, event.readiness().is_readable(), event.readiness().is_writable());
                self.handle_event(event);
                if self.stopped {
                    self.network_thread_sender.send(true);
                    return
                };
                //println!("network thread {} finished handling poll event", &self.addr);
            }
            //println!("network thread {} finished handling ALL poll events", &self.addr);
        }
    }

    fn handle_event(&mut self, event: Event) -> () {
        match event.token() {
            SERVER => {
                // Received an event for the TCP server socket.Accept the connection.
                if let Err(e) = self.accept_stream() {
                    // TODO: Handle Error
                    // println!("error accepting stream:\n {:?}", e);
                };
                // Listen for more
                self.poll.register(&self.listener, SERVER, Ready::readable(), PollOpt::edge() | PollOpt::oneshot());
            }
            DISPATCHER => {
                // Message available from Dispatcher
                self.dispatcher_set_readiness.set_readiness(Ready::empty());
                if let Err(e) = self.receive_dispatch() {
                    // TODO: Handle Error
                    // println!("error receiving from dispatch");
                };
                // We've drained the input_queue, clear the readiness and wait for new notification
                self.poll.register(&self.dispatcher_registration, DISPATCHER, Ready::readable(), PollOpt::edge() | PollOpt::oneshot());
                //println!("network thread {} dispatcher events complete", &self.addr);
            }
            token => {
                //println!("network thread {} handling connection event", &self.addr);
                // lookup token state in kv map <token, state> (it's corresponding addr for now)
                let addr = {
                    if let Some(addr) = self.token_map.get(&token) {addr }
                    else {panic!("does not recognize connection token");}
                };

                if let Some((stream, out_buffer, _, read_buffer, decode_buffer)) = self.stream_map.get_mut(&addr) {
                    if event.readiness().is_writable() {
                        // Connection is writable, try to send the buffer
                        match send_buffer(stream, out_buffer) {
                            Err(ref err) if broken_pipe(err) => {
                                println!("BROKEN PIPE WRITE");
                            },
                            Ok(n) => {
                                self.sent_bytes = self.sent_bytes + n as u64;
                                if !out_buffer.is_empty() {
                                    // Wait for writable again
                                    self.poll.register(stream, token.clone(), Ready::writable(), PollOpt::edge() | PollOpt::oneshot());
                                    //println!("network thread {} buffer is not empty but unable to write any more", &self.addr);
                                }
                            }
                            Err(e) => {
                                println!("Error while sending! {:?}", e);
                            }
                        }

                    }
                    if event.readiness().is_readable() {
                        // Try to fill the decode buffer
                        match Self::receive(stream, read_buffer.as_mut(), decode_buffer) {
                            Ok(0) => {
                                //println!("receive read 0 bytes", read_buffer.capacity(), read_buffer.len());
                                //self.poll.register(stream, token.clone(), Ready::readable(), PollOpt::level() | PollOpt::oneshot());
                                //println!("network thread {} received nothing", stream.local_addr().unwrap());
                            },
                            Ok(n) => {
                                self.received_bytes += n as u64;
                                self.poll.register(stream, token.clone(), Ready::readable(), PollOpt::edge() | PollOpt::oneshot());
                                //println!("network thread {} received {} bytes", stream.local_addr().unwrap(), n);
                                // Try to get frames from the decode buffer
                                self.received_msgs = self.received_msgs + Self::decode(decode_buffer, &mut self.lookup);
                            },
                            Err(ref err) if no_buffer_space(err) => {
                                /*println!("network thread {} no_buffer_space, received bytes: {}\
                                    \n {:?}\
                                    \n read_buffer cap {}, len {}\
                                    \n decode_buffer cap {}, len {}",
                                         self.addr, self.received_bytes, err,
                                         read_buffer.as_mut().capacity(), read_buffer.as_mut().len(),
                                         decode_buffer.as_mut().inner().capacity(), decode_buffer.as_mut().inner().len()
                                );*/
                                let mut new_buffer = BytesMut::new();
                                let mut slice = ([0u8; READ_BUFFER_SIZE]);
                                new_buffer.extend_from_slice(&slice);
                                let mut new_box = Box::new(new_buffer);
                                mem::swap(read_buffer, &mut new_box);

                                // read_buffer replaced, need to make sure the decode_buffer is contiguous:
                                if !(decode_buffer.inner().is_empty()) {
                                    // Take a slice from the head of the new read_buffer
                                    let mut new_decode = read_buffer.split_to(decode_buffer.inner().len());
                                    // Insert the contents of the current decode_buffer into the new one
                                    {
                                        let borrow: &mut [u8] = new_decode.borrow_mut();
                                        borrow.clone_from_slice(decode_buffer.inner().as_mut());
                                    }
                                    decode_buffer.replace_buffer(new_decode);
                                }
                                /*
                                println!("network thread {} shuffled buffers... received bytes: {}\
                                    \n {:?}\
                                    \n read_buffer cap {}, len {}\
                                    \n decode_buffer cap {}, len {}",
                                    self.addr, self.received_bytes, err,
                                    read_buffer.as_mut().capacity(), read_buffer.as_mut().len(),
                                    decode_buffer.as_mut().inner().capacity(), decode_buffer.as_mut().inner().len()
                                );
                                */
                                //self.poll.register(stream, token.clone(), Ready::readable(), PollOpt::edge() | PollOpt::oneshot());
                                self.handle_event(event);
                            }
                            Err(ref err) if broken_pipe(err) => {
                                println!("BROKEN PIPE READ");
                            },
                            Err(err) => {
                                println!("network thread Error while reading, received bytes: {}\n {:?}", self.received_bytes, err);
                            }
                        }

                    }
                }

            }
        }

    }

    fn store_stream(&mut self, stream: TcpStream, addr: &SocketAddr) -> () {
        self.poll.register(&stream, self.token.clone(), Ready::readable() | Ready::writable(), PollOpt::edge() | PollOpt::oneshot());
        // Store the connection
        stream.set_nodelay(true);
        self.token_map.insert(self.token.clone(), addr.clone());
        let mut read_buffer = BytesMut::new();
        let mut slice = ([0u8; READ_BUFFER_SIZE]);
        //println!("extending read_buffer with slice len {}", slice.len());
        read_buffer.extend_from_slice(&slice);
        self.stream_map.insert(
            addr.clone(),
            (
                stream,
                VecDeque::<Box<BytesMut>>::new(),
                self.token.clone(),
                Box::new(read_buffer),  // "Read buffer
                Box::new(DecodeBuffer::new())
            )
        );
        self.next_token();
    }

    fn accept_stream(&mut self) -> io::Result<bool> {
        // Get the details
        while let (stream, addr) = self.listener.accept()? {
            //println!("network thread {} accepted connection from {}", self.addr, addr);
            self.store_stream(stream, &addr);
        }
        Ok(true)
    }

    fn receive_dispatch(&mut self) -> io::Result<bool> {
        while let Ok(event) = self.input_queue.try_recv() {
            match event {
                DispatchEvent::Send(addr, packet) => {
                    self.sent_msgs = self.sent_msgs + 1;
                    //println!("network thread {} handling outgoing packet", &self.addr);
                    // Get the token corresponding to the connection
                    if let Some((stream, buffer, token, _, _)) = self.stream_map.get_mut(&addr) {
                        // The stream is already set-up, buffer the package and try to send the buffer
                        //println!("network thread {} calling send_buffer after receiving message from dispatcher", &self.addr);
                        buffer.push_back(packet);
                        self.poll.register(stream, token.clone(), Ready::writable(), PollOpt::edge() | PollOpt::oneshot());
                        /*send_buffer(stream, buffer);
                        if !buffer.is_empty() {
                        }*/
                    } else {
                        // The stream isn't set-up, request connection, set-it up and try to send the message
                        self.request_stream(addr.clone());
                        if let Some((stream, buffer, _, _, _)) = self.stream_map.get_mut(&addr) {
                            buffer.push_back(packet);
                            //println!("network thread {} calling send_buffer after setting up a stream", &self.addr);
                            send_buffer(stream, buffer);
                        } else {
                            //println!("network thread {} failed to set-up stream to {}", &self.addr, addr);
                        }
                    }
                },
                DispatchEvent::Stop() => {
                    self.stop();
                },
            }
        }
        Ok(true)
    }

    fn request_stream(&mut self, addr: SocketAddr) -> io::Result<bool> {
        //println!("network thread {} requesting connection to {}", self.addr, addr);
        let stream = TcpStream::connect(&addr)?;
        self.store_stream(stream, &addr);
        Ok(true)
    }

    /// Returns `true` if the connection is done.
    fn receive(
        stream: &mut TcpStream,
        read_buffer: &mut BytesMut,
        decode_buffer: &mut DecodeBuffer,
    ) -> io::Result<usize> {
        //println!("network thread {} filling read buffer", stream.local_addr().unwrap());
        // Reserve space in buffer
        //println!("network thread {} init buffer len {}", &self.addr, buf.len());
        let mut read_bytes = 0usize;
        let mut sum_read_bytes = 0usize;
        loop {
            // Keep all the read bytes in the buffer without overwriting
            if read_bytes > 0 {
                //println!("moving {} bytes into received_buffer {}", read_bytes, decode_buffer.inner().len());
                decode_buffer.insert(read_buffer.split_to(read_bytes));
            }
            //read_buffer.reserve(256);
            {
                let borrow: &mut [u8] = read_buffer.borrow_mut();
                //println!("borrow len {}", borrow.len());
                let io_buf = IoVec::from_bytes_mut(borrow);
                if let Some(io_vec) = io_buf {
                    //println!("io_buf len {}", io_vec.len());
                    match stream.read_bufs(&mut [io_vec]) {
                        //match stream.read(read_buffer) {
                        Ok(0) => {
                            //println!("network thread {} reached eof with {} received", stream.local_addr().unwrap(), sum_read_bytes);
                            return Ok(sum_read_bytes) // means caller will listen for more
                            // We reached an end-of-file, time to decode
                        },
                        Ok(n) => {
                            //println!("network thread {} got {} bytes", stream.local_addr().unwrap(), &n);
                            sum_read_bytes = sum_read_bytes + n;
                            read_bytes = n;
                            // continue reading...
                        },
                        // Would block "errors" are the OS's way of saying that the
                        // connection is not actually ready to perform this I/O operation.
                        Err(ref err) if would_block(err) => {
                            //println!("network thread {} would block err", stream.local_addr().unwrap());
                            //###self.poll.register(stream, token.clone(), Ready::readable(), PollOpt::edge() | PollOpt::oneshot());
                            return Ok(sum_read_bytes)
                        },
                        Err(ref err) if interrupted(err) => {
                            //println!("network thread {} interrupted during read", stream.local_addr().unwrap());
                            // We should continue trying until no interruption
                        },
                        // Other errors we'll consider fatal.
                        Err(err) => {
                            //println!("network thread {} got error during read {:?}", stream.local_addr().unwrap(), &err);
                            return Err(err)
                        }
                    }
                } else {
                    return Err(Error::new(ErrorKind::InvalidData, "No space in Buffer"));
                }
            }
        }
    }

    fn decode(decode_buffer: &mut DecodeBuffer, lookup: &mut Arc<ArcSwap<ActorStore>>) -> u64 {
        let mut msgs = 0;
        while let Some(mut frame) = decode_buffer.get_frame() {
            match frame {
                Frame::Data(fr) => {
                    // Forward the data frame to the correct actor
                    let lease_lookup = lookup.lease();
                    {
                        //use bytes::IntoBuf;
                        use dispatch::lookup::ActorLookup;
                        use serialisation::helpers::deserialise_msg;

                        // Consume payload
                        let buf = fr.payload();
                        //let buf = buf.into_buf();
                        let envelope = deserialise_msg(buf).expect("s11n errors");
                        match lease_lookup.get_by_actor_path(envelope.receiver()) {
                            None => {
                                println!(
                                    "Could not find actor reference for destination: {:?}",
                                    envelope.receiver());
                            }
                            Some(actor) => {
                                msgs = msgs +1;
                                //println!("Routing envelope {:?}", envelope);
                                actor.enqueue(envelope);
                            }
                        }
                    }
                },
                _ => {
                    println!("Unexpected frame type");
                },
            }
        };
        return msgs;
    }

    fn stop(&mut self) -> () {
        //println!("Stopping {}", self.addr);
        for (_, (mut stream, mut out_buffer, _, _, _)) in self.stream_map.drain() {
            send_buffer(&mut stream, &mut out_buffer);
            stream.shutdown(Both);
        }
        self.stopped = true;
    }

    fn next_token(&mut self) -> () {
        let next = self.token.0 + 1;
        self.token = Token(next);
    }
}

// Error handling helper functions
fn would_block(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::WouldBlock
}

fn interrupted(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::Interrupted
}

fn no_buffer_space(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::InvalidData
}

fn broken_pipe(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::BrokenPipe
}

fn send_buffer(stream: &mut TcpStream, buffer: &mut VecDeque<Box<BytesMut>>) -> io::Result<usize> {
    let mut sent_bytes = 0;
    while let Some(mut buf) = buffer.pop_front() {
        match stream.write(buf.as_ref()) {
            Ok(n) => {
                sent_bytes = sent_bytes + n;
                //println!("network thread {} sent {} bytes", stream.local_addr().unwrap(), &n);
                if n < buf.as_ref().len() {
                    // Split the data and continue sending the rest later
                    buf.split_to(n);
                    buffer.push_front(buf);
                }
                // Continue looping for the next message
            }
            // Would block "errors" are the OS's way of saying that the
            // connection is not actually ready to perform this I/O operation.
            Err(ref err) if would_block(err) => {
                //println!("network thread would block while sending");
                // re-insert the data at the front of the buffer
                buffer.push_front(buf);
                return Ok(sent_bytes);
            }
            // Got interrupted (how rude!), we'll try again.
            Err(ref err) if interrupted(err) => {
                //println!("network thread interrupted while sending buffer, trying again");
                // re-insert the data at the front of the buffer
                buffer.push_front(buf);
            }
            // Other errors we'll consider fatal.
            Err(err) => {
                //println!("network thread error while sending buffer: {:?}", &err);
                buffer.push_front(buf);
                return Err(err);
            },
        }
    }
    return Ok(sent_bytes);
}

#[cfg(test)]
mod tests {
    use crate::prelude::*;

    use mio::{Registration, Ready};
    use std::sync::mpsc::{channel, Sender, Receiver as Recv};
    use crate::net::network_thread::NetworkThread;
    use std::thread;
    use std::net::{SocketAddr, IpAddr, Ipv4Addr};
    use std::sync::Arc;
    use std::time::Duration;
    use arc_swap::ArcSwap;
    use crate::dispatch::lookup::ActorStore;
    use crate::KompactLogger;

    /* #[test]
    fn network_thread_no_local_poll1() -> () {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 7778);
        let (registration, network_thread_registration) = Registration::new2();
        network_thread_registration.set_readiness(Ready::empty());
        let (sender, receiver) = channel();
        let actor_store = ActorStore::new();
        let lookup = Arc::new(ArcSwap::from(Arc::new(ActorStore::new())));
        let mut network_thread = NetworkThread::new(
            //KompactLogger::new(),
            addr,
            lookup.clone(),
            registration,
            network_thread_registration.clone(),
            receiver,
        );
        thread::spawn(move || {
            network_thread.run();
        });
        //println!("network_thread spawned");
        thread::sleep(Duration::from_secs(1));
        //println!("setting readiness");
        network_thread_registration.set_readiness(Ready::readable());
        thread::sleep(Duration::from_secs(1));
        //println!("clearing readiness");
        network_thread_registration.set_readiness(Ready::empty());
        thread::sleep(Duration::from_secs(1));
        //println!("setting readiness again");
        network_thread_registration.set_readiness(Ready::readable());
        thread::sleep(Duration::from_secs(1));
    } */
}
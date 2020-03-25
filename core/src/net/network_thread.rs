use super::*;
use crate::net::{buffer_pool::BufferPool, ConnectionState};
use mio::{event::Event, net::{TcpListener, TcpStream}, Registry, Events, Poll, Token};
use std::{
    collections::HashMap,
    io,
    net::SocketAddr,
    sync::mpsc::{Receiver as Recv, Sender},
    usize,
    time::Duration,
};
use crate::{
    messaging::{DispatchEnvelope, EventEnvelope},
    net::network_channel::TcpChannel,
};
use fxhash::{FxHashMap, FxHasher};
use std::hash::BuildHasherDefault;

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
const DISPATCHER: Token = Token(usize::MAX - 1);
const MAX_POLL_EVENTS: usize = 1024;
/// How many times to retry on interrupt before we give up
pub const MAX_INTERRUPTS: i32 = 9;
// We do retries when we fail to bind a socket listener during boot-up:
const MAX_BIND_RETRIES: usize = 5;
const BIND_RETRY_INTERVAL: u64 = 1000;

/// Thread structure responsible for driving the Network IO
pub struct NetworkThread {
    log: KompactLogger,
    /// The SocketAddr the network thread is bound to and listening on
    pub addr: SocketAddr,
    //connection_events: UnboundedSender<NetworkEvent>,
    lookup: Arc<ArcSwap<ActorStore>>,
    listener: Option<Box<TcpListener>>,
    poll: Poll,
    // Contains K,V=Remote SocketAddr, Output buffer; Token for polling; Input-buffer,
    channel_map: FxHashMap<SocketAddr, TcpChannel>,
    token_map: FxHashMap<Token, SocketAddr>,
    token: Token,
    input_queue: Box<Recv<DispatchEvent>>,
    dispatcher_ref: DispatcherRef,
    buffer_pool: BufferPool,
    sent_bytes: u64,
    received_bytes: u64,
    sent_msgs: u64,
    stopped: bool,
    network_thread_sender: Sender<bool>,
}

/// Return values for IO Operations on the [NetworkChannel](net::network_channel::NetworkChannel) abstraction
enum IOReturn {
    SwapBuffer,
    Close,
    None,
    Rename(SocketAddr),
}

impl NetworkThread {
    /// Creates a struct for the NetworkThread and binds to a socket without actually spawning a thread.
    /// The `input_queue` is used to send DispatchEvents to the thread but they won't be read unless
    /// the `dispatcher_registration` is activated to wake up the thread.
    /// `network_thread_sender` is used to confirm shutdown of the thread.
    pub fn new(
        log: KompactLogger,
        addr: SocketAddr,
        lookup: Arc<ArcSwap<ActorStore>>,
        input_queue: Recv<DispatchEvent>,
        network_thread_sender: Sender<bool>,
        dispatcher_ref: DispatcherRef,
    ) -> (NetworkThread, Waker) {
        // Set-up the Listener
        debug!(log, "NetworkThread starting, trying to bind listener to address {}", &addr);
        match bind_with_retries(&addr, MAX_BIND_RETRIES, &log) {
            Ok(mut listener) => {
                let actual_addr = listener.local_addr().expect("could not get real addr");

                // Set up polling for the Dispatcher and the listener.
                let poll = Poll::new().expect("failed to create Poll instance in NetworkThread");

                // Register Listener
                let _ = poll.registry().register(
                    &mut listener,
                    SERVER,
                    Interest::READABLE,
                )
                    .expect("failed to register TCP SERVER");

                // Create waker for Dispatch
                let mut waker = Waker::new(poll.registry(), DISPATCHER)
                    .expect("failed to create Waker for DISPATCHER");

                pub type FxBuildHasher = BuildHasherDefault<FxHasher>;
                let channel_map: FxHashMap<SocketAddr, TcpChannel> =
                    HashMap::<SocketAddr, TcpChannel, FxBuildHasher>::default();
                let token_map: FxHashMap<Token, SocketAddr> =
                    HashMap::<Token, SocketAddr, FxBuildHasher>::default();

                (NetworkThread {
                    log,
                    addr: actual_addr,
                    lookup,
                    listener: Some(Box::new(listener)),
                    poll,
                    channel_map,
                    token_map,
                    token: START_TOKEN,
                    input_queue: Box::new(input_queue),
                    buffer_pool: BufferPool::new(),
                    sent_bytes: 0,
                    received_bytes: 0,
                    sent_msgs: 0,
                    stopped: false,
                    network_thread_sender,
                    dispatcher_ref,
                }, waker)
            }
            Err(e) => {
                panic!("NetworkThread failed to bind to address: {:?}", e);
            }
        }
    }


    /// Event loop, spawn a thread calling this method start the thread.
    pub fn run(&mut self) -> () {
        let mut events = Events::with_capacity(MAX_POLL_EVENTS);
        debug!(self.log, "NetworkThread entering main EventLoop");
        loop {
            self.poll
                .poll(&mut events, None)
                .expect("Error when calling Poll");

            for event in events.iter() {
                if let Err(e) = self.handle_event(event) {
                    error!(self.log, "NetworkThread Error while handling event with token {:?}: {:?}", event.token(), e);
                };
                if self.stopped {
                    if let Err(e) = self.network_thread_sender.send(true) {
                        error!(self.log, "NetworkThread error shutting down sender: {:?}", e);
                    };
                    debug!(self.log, "NetworkThread {} Stopped", self.addr);
                    return;
                };
            }
        }
    }

    fn handle_event(&mut self, event: &Event) -> io::Result<()> {
        match event.token() {
            SERVER => {
                // Received an event for the TCP server socket.Accept the connection.
                if let Err(e) = self.accept_stream() {
                    debug!(self.log, "NetworkThread {} Error while accepting stream {:?}",
                           self.addr,
                           e
                    );
                }
            }
            DISPATCHER => {
                // Message available from Dispatcher, clear the poll readiness before receiving
                self.receive_dispatch()?;
            }
            token => {
                // lookup token state in kv map <token, state> (it's corresponding addr for now)
                let mut addr = {
                    if let Some(addr) = self.token_map.get(&token) {
                        addr.clone()
                    } else {
                        debug!(self.log, "NetworkThread {} Poll triggered for removed channel, Token({})", self.addr, token.0);
                        return Ok(());
                    }
                };
                let mut swap_buffer = false;
                let mut close_channel = false;
                if event.is_writable() {
                    self.try_write(&addr)
                }
                if event.is_readable() {
                    match self.try_read(&addr) {
                        IOReturn::Close => {
                            // Remove and deregister
                            close_channel = true;
                        }
                        IOReturn::SwapBuffer => {
                            swap_buffer = true;
                        }
                        _ => {}
                    }
                    match self.decode(&addr) {
                        IOReturn::Rename(new_addr) => {
                            self.rename(event.token(), new_addr);
                            addr = new_addr;
                        }
                        IOReturn::Close => {
                            // Remove and deregister
                            close_channel = true;
                        }
                        _ => {}
                    }
                    if close_channel {
                        if let Some(mut channel) = self.channel_map.remove(&addr) {
                            self.poll.registry().deregister(channel.stream_mut());
                            channel.shutdown();
                            drop(channel);
                        }
                        // Tell the dispatcher that we've closed the connection
                        self.dispatcher_ref.tell(DispatchEnvelope::Event(EventEnvelope::Network(
                            NetworkEvent::Connection(addr, ConnectionState::Closed),
                        )));
                        return Ok(());
                    }
                    if swap_buffer {
                        // Buffer full, we swap it and register for poll again
                        if let Some(channel) = self.channel_map.get_mut(&addr) {
                            if let Some(mut new_buffer) = self.buffer_pool.get_buffer() {
                                channel.swap_buffer(&mut new_buffer);
                                debug!(self.log, "NetworkThread {} swapped buffer for channel {} with msg count {}", self.addr, &addr, channel.messages);
                                self.buffer_pool.return_buffer(new_buffer);
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn rename(&mut self, token: Token, hello_addr: SocketAddr) -> () {
        if let Some(registered_addr) = self.token_map.remove(&token) {
            debug!(
                self.log,
                "NetworkThread {} got Hello({}) from {}", self.addr, &hello_addr, &registered_addr
            );
            if hello_addr == registered_addr {
                // The channel is already correct, we update state at the end
            } else {
                // Make sure we only have one channel and that it's registered with the hello_addr
                if let Some(mut channel) = self.channel_map.remove(&hello_addr) {
                    // There's a channel registered with the hello_addr

                    if let Some(mut other_channel) = self.channel_map.remove(&registered_addr) {
                        // There's another channel, we received a Hello message on a different channel.
                        // This is not good. We need to merge the two connections into one.
                        debug!(
                            self.log,
                            "NetworkThread {} merging {} into {}",
                            self.addr,
                            &registered_addr,
                            &hello_addr
                        );
                        // Merge will check the SocketAddr of the two systems and uniformly decide which to keep in &channel
                        channel.merge(&mut other_channel);
                        self.channel_map.insert(hello_addr, channel);
                    } else {
                        // Expected case, no merge needed
                        self.channel_map.insert(hello_addr, channel);
                    }
                } else if let Some(channel) = self.channel_map.remove(&registered_addr) {
                    // Re-insert the channel with different key
                    self.channel_map.insert(hello_addr, channel);
                }
            }

            // Make sure that the channel is registered correctly and in Connected State.
            if let Some(channel) = self.channel_map.get_mut(&hello_addr) {
                channel.state = ConnectionState::Connected(hello_addr);
                channel.token = token.clone();
                self.token_map.insert(token, hello_addr);
                self.dispatcher_ref
                    .tell(DispatchEnvelope::Event(EventEnvelope::Network(
                        NetworkEvent::Connection(hello_addr, ConnectionState::Connected(hello_addr)),
                    )));
            }
        } else {
            panic!("No address registered for a token which yielded a hello msg");
        }
    }

    fn try_write(&mut self, addr: &SocketAddr) -> () {
        if let Some(channel) = self.channel_map.get_mut(&addr) {
            match channel.try_drain() {
                Err(ref err) if broken_pipe(err) => {
                    // Remove the channel and notify the dispatcher
                    self.channel_map.remove(&addr);
                    self.dispatcher_ref.tell(DispatchEnvelope::Event(EventEnvelope::Network(
                        NetworkEvent::Connection(addr.clone(), ConnectionState::Closed),
                    )));
                }
                Ok(n) => {
                    self.sent_bytes = self.sent_bytes + n as u64;
                }
                Err(e) => {
                    error!(self.log, "NetworkThread {} unhandled error while writing {:?}", self.addr, e)
                }
            }
        }
    }

    fn try_read(&mut self, addr: &SocketAddr) -> IOReturn {
        let mut ret = IOReturn::None;
        if let Some(channel) = self.channel_map.get_mut(&addr) {
            match channel.receive() {
                Ok(n) => {
                    self.received_bytes += n as u64;
                }
                Err(ref err) if no_buffer_space(err) => {
                    debug!(self.log, "NetworkThread {} no_buffer_space for channel to {}", self.addr, &addr);
                    ret = IOReturn::SwapBuffer
                }
                Err(err) if interrupted(&err) || would_block(&err) => {
                    // Just retry later
                }
                Err(err) if connection_reset(&err) => {
                    debug!(self.log, "NetworkThread {} Connection_reset to peer {}, shutting down the channel", self.addr, &addr);
                    channel.shutdown();
                    ret = IOReturn::Close
                }
                Err(err) => {
                    // Fatal error don't try to read again
                    error!(self.log, "NetworkThread {} Error while reading from peer {}:\n{:?}", self.addr, &addr, &err);
                }
            }
        }
        ret
    }

    fn decode(&mut self, addr: &SocketAddr) -> IOReturn {
        let mut ret = IOReturn::None;
        if let Some(channel) = self.channel_map.get_mut(addr) {
            while let Some(frame) = channel.decode() {
                match frame {
                    Frame::Data(fr) => {
                        // Forward the data frame to the correct actor
                        let lease_lookup = self.lookup.lease();
                        {
                            use dispatch::lookup::ActorLookup;
                            use serialisation::ser_helpers::deserialise_msg;
                            let buf = fr.payload();
                            let envelope = deserialise_msg(buf).expect("s11n errors");
                            match lease_lookup.get_by_actor_path(envelope.receiver()) {
                                None => {
                                    debug!(&self.log, "NetworkThread {} Could not find actor reference for destination: {:?}, dropping message", self.addr, envelope.receiver()
                                    )
                                }
                                Some(actor) => {
                                    actor.enqueue(envelope);
                                }
                            }
                        }
                    }
                    Frame::Hello(hello) => {
                        ret = IOReturn::Rename(hello.addr)
                    }
                    Frame::Bye() => {
                        debug!(self.log, "NetworkThread {} received Bye from {}", self.addr, &addr);
                        self.dispatcher_ref
                            .tell(DispatchEnvelope::Event(EventEnvelope::Network(
                                NetworkEvent::Connection(*addr, ConnectionState::Closed),
                            )));
                        return IOReturn::Close;
                    }
                    _ => {
                        error!(self.log, "NetworkThread {} unexpected frame type from {}", self.addr, &addr);
                    }
                }
            }
        }
        return ret;
    }

    fn request_stream(&mut self, addr: SocketAddr) -> io::Result<()> {
        // Make sure we never request request a stream to someone we already have a connection to
        // Async communication with the dispatcher can lead to this
        if let Some(_) = self.channel_map.get(&addr) {
            // We already have a connection set-up
            // the connection request must have been sent before the channel was initialized
            debug!(self.log, "NetworkThread {} asked to request connection to already connected host {}", self.addr, &addr);
            return Ok(());
        }
        debug!(self.log, "NetworkThread {} requesting connection to {}", self.addr, &addr);
        match TcpStream::connect(addr.clone()) {
            Ok(stream) => {
                self.store_stream(stream, &addr)?;
                Ok(())
            }
            Err(e) => {
                error!(self.log, "NetworkThread {} failed to connect to remote host {}, error: {:?}", addr, &addr, e);
                Ok(())
            }
        }
    }

    #[allow(irrefutable_let_patterns)]
    fn accept_stream(&mut self) -> io::Result<()> {
        while let (stream, addr) = (self.listener.as_ref().unwrap()).accept()? {
            debug!(self.log, "NetworkThread {} accepting connection from {}", self.addr, &addr);
            self.store_stream(stream, &addr)?;
        }
        Ok(())
    }

    fn store_stream(&mut self, stream: TcpStream, addr: &SocketAddr) -> io::Result<()> {
        if let Some(buffer) = self.buffer_pool.get_buffer() {
            self.token_map.insert(self.token.clone(), addr.clone());
            let mut channel = TcpChannel::new(stream, self.token, buffer);
            debug!(self.log, "NetworkThread {} saying Hello to {}", self.addr, &addr);
            let _ = channel.initialize(&self.addr);
            let _ = self.poll.registry().register(
                channel.stream_mut(),
                self.token.clone(),
                Interest::READABLE | Interest::WRITABLE,
            );
            self.channel_map.insert(addr.clone(), channel);
            self.next_token();
            Ok(())
        } else {
            // TODO: Handle BufferPool running out much better.
            panic!("Unable to store a stream, no buffers available!");
        }
    }

    fn receive_dispatch(&mut self) -> io::Result<()> {
        while let Ok(event) = self.input_queue.try_recv() {
            match event {
                DispatchEvent::Send(addr, frame) => {
                    self.sent_msgs += 1;
                    // Get the token corresponding to the connection
                    if let Some(channel) = self.channel_map.get_mut(&addr) {
                        // The stream is already set-up, buffer the package and wait for writable event
                        channel.enqueue_serialized(frame);
                        if let Err(e) = channel.try_drain() {
                            if broken_pipe(&e) {
                                self.channel_map.remove(&addr);
                                error!(self.log, "Network Thread {} Broken pipe sending to {}: {:?}", self.addr, &addr, e);
                                self.dispatcher_ref.tell(DispatchEnvelope::Event(EventEnvelope::Network(
                                    NetworkEvent::Connection(addr.clone(), ConnectionState::Closed))));
                                // Continue with the input_queue...
                                break;
                            }
                            error!(self.log, "error when sending newly enqueued message: {:?}", e);
                        }
                    } else {
                        // The stream isn't set-up, request connection, set-it up and try to send the message
                        debug!(self.log, "Dispatch trying to route to unrecognized address {}, rejecting the message", addr);
                        self.dispatcher_ref.tell(DispatchEnvelope::Event(EventEnvelope::Network(
                            NetworkEvent::RejectedFrame(addr.clone(), frame))));
                    }
                }
                DispatchEvent::Stop() => {
                    self.stop();
                }
                DispatchEvent::Connect(addr) => {
                    debug!(
                        self.log,
                        "NetworkThread got DispatchEvent::Connect({})", addr
                    );
                    self.request_stream(addr.clone())?;
                }
            }
        }
        Ok(())
    }

    fn stop(&mut self) -> () {
        let tokens = self.token_map.clone();
        for (_, addr) in tokens {
            self.try_read(&addr);
        }
        for (_, mut channel) in self.channel_map.drain() {
            debug!(
                self.log,
                "NetworkThread {} Stopping channel with message count {}",
                self.addr,
                channel.messages
            );
            match channel.state {
                ConnectionState::Closed => {}
                _ => {
                    channel.graceful_shutdown();
                }
            }
        }
        if let Some(mut listener) = self.listener.take() {
            self.poll.registry().deregister(&mut listener);
            drop(listener);
            debug!(
                self.log,
                "NetworkThread {} Dropped its listener and will now stop",
                self.addr,
            );
        }
        self.stopped = true;
    }

    fn next_token(&mut self) -> () {
        let next = self.token.0 + 1;
        self.token = Token(next);
    }
}

fn bind_with_retries(addr: &SocketAddr, retries: usize, log: &KompactLogger) -> io::Result<TcpListener> {
    match TcpListener::bind(addr.clone()) {
        Ok(listener) => { Ok(listener) }
        Err(e) => {
            if retries > 0 {
                debug!(log, "Failed to bind to addr {}, will retry {} more times, error was: {:?}", addr, retries, e);
                // Lets give cleanup some time to do it's thing before we retry
                thread::sleep(Duration::from_millis(BIND_RETRY_INTERVAL));
                bind_with_retries(addr, retries - 1, log)
            } else {
                Err(e)
            }
        }
    }
}

// Error handling helper functions
pub(crate) fn would_block(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::WouldBlock
}

pub(crate) fn interrupted(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::Interrupted
}

pub(crate) fn no_buffer_space(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::InvalidData
}

pub(crate) fn connection_reset(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::ConnectionReset
}

pub(crate) fn broken_pipe(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::BrokenPipe
}

#[cfg(test)]
#[allow(unused_must_use)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};
    use crate::dispatch::NetworkConfig;


    /*
    Needs to be re-written for MIO 0.7.0, not worth it, test is too specific anyway
    #[allow(unused_must_use)]
    fn setup_two_threads(addr1: SocketAddr, addr2: SocketAddr) -> (NetworkThread, Sender<DispatchEvent>, NetworkThread, Sender<DispatchEvent>) {
        let mut cfg = KompactConfig::new();
        cfg.system_components(DeadletterBox::new, NetworkConfig::default().build());
        let system = cfg.build().expect("KompactSystem");

        // Set-up the the threads arguments
        let lookup = Arc::new(ArcSwap::from(Arc::new(ActorStore::new())));
        let (registration1, network_thread_registration1) = Registration::new2();
        let (registration2, network_thread_registration2) = Registration::new2();
        //network_thread_registration.set_readiness(Interest::empty());
        let (input_queue_1_sender, input_queue_1_receiver) = channel();
        let (input_queue_2_sender, input_queue_2_receiver) = channel();
        let (dispatch_shutdown_sender1, _) = channel();
        let (dispatch_shutdown_sender2, _) = channel();
        let logger = system.logger().clone();
        let dispatcher_ref = system.dispatcher_ref();

        // Set up the two network threads
        let network_thread1 = NetworkThread::new(
            logger.clone(),
            addr1.clone(),
            lookup.clone(),
            registration1,
            network_thread_registration1.clone(),
            input_queue_1_receiver,
            dispatch_shutdown_sender1,
            dispatcher_ref.clone(),
        );

        let network_thread2 = NetworkThread::new(
            logger.clone(),
            addr2.clone(),
            lookup.clone(),
            registration2,
            network_thread_registration2.clone(),
            input_queue_2_receiver,
            dispatch_shutdown_sender2,
            dispatcher_ref.clone(),
        );

        (network_thread1, input_queue_1_sender, network_thread2, input_queue_2_sender)
    }


    #[test]
    fn merge_connections_basic() -> () {
        // Sets up two NetworkThreads and does mutual connection request

        let addr1 = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 7778);
        let addr2 = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 7780);

        let (mut network_thread1, input_queue_1_sender, mut network_thread2, input_queue_2_sender) = setup_two_threads(addr1.clone(), addr2.clone());

        // Tell both to connect to each-other before they start running:
        input_queue_1_sender.send(DispatchEvent::Connect(addr2.clone()));
        input_queue_2_sender.send(DispatchEvent::Connect(addr1.clone()));

        // Let both handle the connect event:
        network_thread1.receive_dispatch();
        network_thread2.receive_dispatch();

        // Wait for the connect requests to reach destination:
        thread::sleep(Duration::from_millis(500));

        // Accept requested streams
        network_thread1.accept_stream();
        network_thread2.accept_stream();

        // Wait for Hello to reach destination:
        thread::sleep(Duration::from_millis(500));

        // We need to make sure the TCP buffers are actually flushing the messages. Give both channels two cycles to finnish
        // Cycle one Requested channels
        network_thread1.handle_event(Event::new(Interest::readable() | Interest::writable(), START_TOKEN));
        thread::sleep(Duration::from_millis(100));
        network_thread2.handle_event(Event::new(Interest::readable() | Interest::writable(), START_TOKEN));
        thread::sleep(Duration::from_millis(100));

        // Cycle one Accepted channels:
        network_thread1.handle_event(Event::new(Interest::readable() | Interest::writable(), Token(START_TOKEN.0 + 1)));
        thread::sleep(Duration::from_millis(100));
        network_thread2.handle_event(Event::new(Interest::readable() | Interest::writable(), Token(START_TOKEN.0 + 1)));
        thread::sleep(Duration::from_millis(100));

        // Cycle two Requested channels
        network_thread1.handle_event(Event::new(Interest::readable() | Interest::writable(), START_TOKEN));
        thread::sleep(Duration::from_millis(100));
        network_thread2.handle_event(Event::new(Interest::readable() | Interest::writable(), START_TOKEN));
        thread::sleep(Duration::from_millis(100));

        // Cycle two Accepted channels:
        network_thread1.handle_event(Event::new(Interest::readable() | Interest::writable(), Token(START_TOKEN.0 + 1)));
        thread::sleep(Duration::from_millis(100));
        network_thread2.handle_event(Event::new(Interest::readable() | Interest::writable(), Token(START_TOKEN.0 + 1)));
        thread::sleep(Duration::from_millis(100));

        // Now we can inspect the Network channels, both only have one channel:
        assert_eq!(network_thread1.channel_map.len(), 1);
        assert_eq!(network_thread2.channel_map.len(), 1);

        // Now assert that they've kept the same channel:
        assert_eq!(network_thread1.channel_map.drain().next().unwrap().1.stream().local_addr().unwrap(),
                   network_thread2.channel_map.drain().next().unwrap().1.stream().peer_addr().unwrap());
    }

    #[test]
    fn merge_connections_tricky() -> () {
        // Sets up two NetworkThreads and does mutual connection request
        // This test uses a different order of events than basic

        let addr1 = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8778);
        let addr2 = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8780);

        let (mut network_thread1, input_queue_1_sender, mut network_thread2, input_queue_2_sender) = setup_two_threads(addr1.clone(), addr2.clone());

        // 2 Requests connection to 1 and sends Hello
        input_queue_2_sender.send(DispatchEvent::Connect(addr1.clone()));
        network_thread2.receive_dispatch();
        thread::sleep(Duration::from_millis(100));

        // 1 accepts the connection and sends hello back
        network_thread1.accept_stream();
        thread::sleep(Duration::from_millis(100));

        // 2 receives the Hello
        network_thread2.handle_event(Event::new(Interest::readable() | Interest::writable(), START_TOKEN));
        thread::sleep(Duration::from_millis(100));

        // 1 Receives Hello
        network_thread1.handle_event(Event::new(Interest::readable() | Interest::writable(), START_TOKEN));

        // 1 Receives Request Connection Event, this is the tricky part
        // 1 Requests connection to 2 and sends Hello
        input_queue_1_sender.send(DispatchEvent::Connect(addr2.clone()));
        network_thread1.receive_dispatch();
        thread::sleep(Duration::from_millis(100));

        // 2 accepts the connection and replies with hello
        network_thread2.accept_stream();
        thread::sleep(Duration::from_millis(100));

        // 2 receives the Hello on the new channel and merges
        network_thread2.handle_event(Event::new(Interest::readable() | Interest::writable(), Token(START_TOKEN.0 + 1)));
        thread::sleep(Duration::from_millis(100));

        // 1 receives the Hello on the new channel and merges
        network_thread1.handle_event(Event::new(Interest::readable() | Interest::writable(), Token(START_TOKEN.0 + 1)));
        thread::sleep(Duration::from_millis(100));

        // Now we can inspect the Network channels, both only have one channel:
        assert_eq!(network_thread1.channel_map.len(), 1);
        assert_eq!(network_thread2.channel_map.len(), 1);

        // Now assert that they've kept the same channel:
        assert_eq!(network_thread1.channel_map.drain().next().unwrap().1.stream().local_addr().unwrap(),
                   network_thread2.channel_map.drain().next().unwrap().1.stream().peer_addr().unwrap());
    }

    #[test]
    fn graceful_network_shutdown() -> () {
        // Sets up two NetworkThreads and connects them to eachother, then shuts it down

        let addr1 = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8878);
        let addr2 = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8880);

        let (mut network_thread1, input_queue_1_sender, mut network_thread2, input_queue_2_sender) = setup_two_threads(addr1.clone(), addr2.clone());

        // 2 Requests connection to 1 and sends Hello
        input_queue_2_sender.send(DispatchEvent::Connect(addr1.clone()));
        network_thread2.receive_dispatch();
        thread::sleep(Duration::from_millis(100));
        network_thread1.accept_stream();
        // Hello is sent to network_thread2, let it read:
        network_thread2.handle_event(Event::new(Interest::readable() | Interest::writable(), START_TOKEN));
        //
    }
    */
}

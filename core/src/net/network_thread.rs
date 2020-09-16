use super::*;
use crate::{
    dispatch::NetworkConfig,
    messaging::{DispatchEnvelope, EventEnvelope},
    net::{
        buffer_pool::BufferPool,
        network_channel::{ChannelState, TcpChannel},
        udp_state::UdpState,
        ConnectionState,
    },
};
use crossbeam_channel::Receiver as Recv;
use mio::{
    event::Event,
    net::{TcpListener, TcpStream, UdpSocket},
    Events,
    Poll,
    Token,
};
use rustc_hash::FxHashMap;
use std::{io, net::SocketAddr, sync::Arc, time::Duration, usize};
use uuid::Uuid;

/*
    Using https://github.com/tokio-rs/mio/blob/master/examples/tcp_server.rs as template.
    Single threaded MIO event loop.
    Receives outgoing Box<IoVec> via SegQueue, notified by polling via the "Registration"
    Buffers the outgoing Boxes for different connections in a hashmap.
    Will Send incoming messages directly to components.
*/

// Used for identifying connections
const TCP_SERVER: Token = Token(0);
const UDP_SOCKET: Token = Token(1);
// Used for identifying the dispatcher/input queue
const DISPATCHER: Token = Token(2);
const START_TOKEN: Token = Token(3);
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
    tcp_listener: Option<TcpListener>,
    udp_state: Option<UdpState>,
    poll: Poll,
    // Contains K,V=Remote SocketAddr, Output buffer; Token for polling; Input-buffer,
    channel_map: FxHashMap<SocketAddr, TcpChannel>,
    token_map: FxHashMap<Token, SocketAddr>,
    token: Token,
    input_queue: Recv<DispatchEvent>,
    dispatcher_ref: DispatcherRef,
    buffer_pool: BufferPool,
    sent_bytes: u64,
    received_bytes: u64,
    sent_msgs: u64,
    stopped: bool,
    shutdown_promise: Option<KPromise<()>>,
}

/// Return values for IO Operations on the [NetworkChannel](net::network_channel::NetworkChannel) abstraction
#[derive(Debug, PartialEq, Eq)]
pub(super) enum IOReturn {
    SwapBuffer,
    Close,
    None,
    Start(SocketAddr, Uuid),
    Ack,
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
        shutdown_promise: KPromise<()>,
        dispatcher_ref: DispatcherRef,
        config: NetworkConfig,
    ) -> (NetworkThread, Waker) {
        // Set-up the Listener
        debug!(
            log,
            "NetworkThread starting, trying to bind listener to address {}", &addr
        );
        match bind_with_retries(&addr, MAX_BIND_RETRIES, &log) {
            Ok(mut tcp_listener) => {
                let actual_addr = tcp_listener.local_addr().expect("could not get real addr");
                let logger = log.new(o!("addr" => format!("{}", actual_addr)));
                let mut udp_socket =
                    UdpSocket::bind(actual_addr).expect("could not bind UDP on TCP port");

                // Set up polling for the Dispatcher and the listener.
                let poll = Poll::new().expect("failed to create Poll instance in NetworkThread");

                // Register Listener
                let registry = poll.registry();
                registry
                    .register(&mut tcp_listener, TCP_SERVER, Interest::READABLE)
                    .expect("failed to register TCP SERVER");
                registry
                    .register(
                        &mut udp_socket,
                        UDP_SOCKET,
                        Interest::READABLE | Interest::WRITABLE,
                    )
                    .expect("failed to register UDP SOCKET");

                // Create waker for Dispatch
                let waker = Waker::new(poll.registry(), DISPATCHER)
                    .expect("failed to create Waker for DISPATCHER");

                let mut buffer_pool = BufferPool::with_config(
                    &config.get_buffer_config(),
                    &config.get_custom_allocator(),
                );

                let udp_buffer = buffer_pool
                    .get_buffer()
                    .expect("Could not get buffer for setting up UDP");

                let udp_state = UdpState::new(udp_socket, udp_buffer, logger.clone());
                let channel_map: FxHashMap<SocketAddr, TcpChannel> = FxHashMap::default();
                let token_map: FxHashMap<Token, SocketAddr> = FxHashMap::default();

                (
                    NetworkThread {
                        log: logger,
                        addr: actual_addr,
                        lookup,
                        tcp_listener: Some(tcp_listener),
                        udp_state: Some(udp_state),
                        poll,
                        channel_map,
                        token_map,
                        token: START_TOKEN,
                        input_queue,
                        buffer_pool,
                        sent_bytes: 0,
                        received_bytes: 0,
                        sent_msgs: 0,
                        stopped: false,
                        shutdown_promise: Some(shutdown_promise),
                        dispatcher_ref,
                    },
                    waker,
                )
            }
            Err(e) => {
                panic!(
                    "NetworkThread failed to bind to address: {:?}, addr {:?}",
                    e, &addr
                );
            }
        }
    }

    /// Event loop, spawn a thread calling this method start the thread.
    pub fn run(&mut self) -> () {
        let mut events = Events::with_capacity(MAX_POLL_EVENTS);
        debug!(self.log, "Entering main EventLoop");
        loop {
            self.poll
                .poll(&mut events, None)
                .expect("Error when calling Poll");

            for event in events.iter() {
                if let Err(e) = self.handle_event(event) {
                    error!(
                        self.log,
                        "Error while handling event with token {:?}: {:?}",
                        event.token(),
                        e
                    );
                };
                if self.stopped {
                    let promise = self.shutdown_promise.take().expect("shutdown promise");
                    if let Err(e) = promise.fulfil(()) {
                        error!(self.log, "Error, shutting down sender: {:?}", e);
                    };
                    debug!(self.log, "Stopped");
                    return;
                };
            }
        }
    }

    fn handle_event(&mut self, event: &Event) -> io::Result<()> {
        match event.token() {
            TCP_SERVER => {
                // Received an event for the TCP server socket.Accept the connection.
                if let Err(e) = self.accept_stream() {
                    debug!(self.log, "Error while accepting stream {:?}", e);
                }
            }
            UDP_SOCKET => {
                if let Some(ref mut udp_state) = self.udp_state {
                    if event.is_writable() {
                        match udp_state.try_write() {
                            Ok(n) => {
                                self.sent_bytes += n as u64;
                            }
                            Err(e) => {
                                warn!(self.log, "Error during UDP sending: {}", e);
                            }
                        }
                    }
                    if event.is_readable() {
                        match udp_state.try_read() {
                            Ok((n, ioret)) => {
                                if n > 0 {
                                    self.received_bytes += n as u64;
                                }
                                if IOReturn::SwapBuffer == ioret {
                                    if let Some(mut new_buffer) = self.buffer_pool.get_buffer() {
                                        udp_state.swap_buffer(&mut new_buffer);
                                        self.buffer_pool.return_buffer(new_buffer);
                                        debug!(self.log, "Swapped UDP buffer");
                                    // We handle the event again, must perform read again somehow!!!
                                    } else {
                                        error!(self.log, "Could not get UDP buffer",);
                                    }
                                    return self.handle_event(event);
                                }
                            }
                            Err(e) => {
                                warn!(self.log, "Error during UDP reading: {}", e);
                            }
                        }
                        use dispatch::lookup::ActorLookup;

                        // Forward the data frame to the correct actor
                        let lease_lookup = self.lookup.load();
                        for envelope in udp_state.incoming_messages.drain(..) {
                            match lease_lookup.get_by_actor_path(&envelope.receiver) {
                            Some(actor) => {
                                actor.enqueue(envelope);
                            }
                            None => {
                                debug!(self.log, "Could not find actor reference for destination: {:?}, dropping message", envelope.receiver)
                            }
                        }
                        }
                    }
                } else {
                    debug!(self.log, "Poll triggered for removed UDP socket");
                    return Ok(());
                }
            }
            DISPATCHER => {
                // Message available from Dispatcher, clear the poll readiness before receiving
                self.receive_dispatch()?;
            }
            token => {
                // lookup its corresponding addr
                let addr = {
                    if let Some(addr) = self.token_map.get(&token) {
                        *addr
                    } else {
                        debug!(
                            self.log,
                            "Poll triggered for removed channel, Token({})", token.0
                        );
                        return Ok(());
                    }
                };
                let mut swap_buffer = false;
                let mut close_channel = false;
                if event.is_writable() {
                    if let IOReturn::Close = self.try_write(&addr) {
                        // Remove and deregister
                        close_channel = true;
                    }
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
                        IOReturn::Start(remote_addr, id) => {
                            self.handle_start(event.token(), remote_addr, id);
                        }
                        IOReturn::Close => {
                            // Remove and deregister
                            close_channel = true;
                        }
                        _ => (),
                    }
                    if close_channel {
                        self.close_channel(addr);
                        // Tell the dispatcher that we've closed the connection
                        return Ok(());
                    }
                    if swap_buffer {
                        // Buffer full, we swap it and register for poll again
                        if let Some(channel) = self.channel_map.get_mut(&addr) {
                            if let Some(mut new_buffer) = self.buffer_pool.get_buffer() {
                                channel.swap_buffer(&mut new_buffer);
                                self.buffer_pool.return_buffer(new_buffer);
                                debug!(self.log, "Swapped buffer for {:?}", &channel);
                            // We handle the event again, must perform read again somehow!!!
                            } else {
                                error!(self.log, "Could not get buffer for channel {}", &addr);
                            }
                        }
                        // We retry the event such that the read is performed with the new buffer.
                        return self.handle_event(event);
                    }
                }
            }
        }
        Ok(())
    }

    /// During channel initialization the threeway handshake to establish connections culminates with this function
    /// The Start(remote_addr, id) is received by the host on the receiving end of the channel initialisation.
    /// The decision is made here and now.
    /// If no other connection is registered for the remote host the decision is easy, we start the channel and send the ack.
    /// If there are other connection attempts underway there are multiple possibilities:
    ///     The other connection has not started and does not have a known UUID: it will be killed, this channel will start.
    ///     The connection has already started, in which case this channel must be killed.
    ///     The connection has a known UUID but is not connected: Use the UUID as a tie breaker for which to kill and which to keep.
    fn handle_start(&mut self, token: Token, remote_addr: SocketAddr, id: Uuid) -> () {
        if let Some(registered_addr) = self.token_map.remove(&token) {
            if remote_addr == registered_addr {
                debug!(
                    self.log,
                    "Got Start({}, ...) from {}, already registered with correct addr",
                    &remote_addr,
                    &registered_addr
                );
            // The channel we received the start on was already registered with the appropriate address.
            // There is no need to change anything, we can simply transition the channel.
            } else {
                // Make sure we only have one channel and that it's registered with the remote_addr
                if let Some(mut channel) = self.channel_map.remove(&registered_addr) {
                    // There's a channel registered with the remote_addr

                    if let Some(mut other_channel) = self.channel_map.remove(&remote_addr) {
                        // There's another channel for the same host, only one can survive.
                        // If we don't knw the Uuid yet the channel can safely be killed. The remote host must obey our Ack.
                        // It can not discard the other channel without receiving a Start(...) or Ack(...) for the other channel.
                        if let Some(other_id) = other_channel.get_id() {
                            // The other channel has a known id, if it doesn't there is no reason to keep it.

                            if other_channel.connected() || other_id > id {
                                // The other channel should be kept and this one should be discarded.
                                debug!(
                                    self.log,
                                    "Got Start({}, ...) from {}, already connected",
                                    &remote_addr,
                                    &registered_addr
                                );
                                let _ = self.poll.registry().deregister(channel.stream_mut());
                                channel.graceful_shutdown();
                                self.channel_map.insert(remote_addr, other_channel);
                                // It will be driven to completion on its own.
                                return;
                            }
                        }
                        // We will keep this channel, not the other channel
                        info!(
                            self.log,
                            "Dropping other_channel while starting channel {}", &remote_addr
                        );
                        let _ = self
                            .poll
                            .registry()
                            .deregister(other_channel.stream_mut())
                            .ok();
                        other_channel.shutdown();
                        drop(other_channel);
                        // Continue with `channel`
                    }
                    // Re-insert the channel and continue starting it.
                    self.channel_map.insert(remote_addr, channel);
                } else if let Some(channel) = self.channel_map.remove(&remote_addr) {
                    // Only one channel, re-insert the channel with the correct key
                    debug!(
                        self.log,
                        "Got Start({}, ...) from {}, changing name of channel.",
                        &remote_addr,
                        &registered_addr
                    );
                    self.channel_map.insert(remote_addr, channel);
                }
            }

            // Make sure that the channel is registered correctly and in Connected State.
            if let Some(channel) = self.channel_map.get_mut(&remote_addr) {
                debug!(
                    self.log,
                    "Sending ack for {}, {}", &remote_addr, &channel.token.0
                );
                channel.handle_start(&remote_addr, id);
                channel.token = token;
                self.token_map.insert(token, remote_addr);
                if let Err(e) = self.poll.registry().reregister(
                    channel.stream_mut(),
                    token,
                    Interest::WRITABLE | Interest::READABLE,
                ) {
                    warn!(
                        self.log,
                        "Error when reregistering Poll for channel in handle_hello: {:?}", e
                    );
                };

                self.dispatcher_ref
                    .tell(DispatchEnvelope::Event(EventEnvelope::Network(
                        NetworkEvent::Connection(
                            remote_addr,
                            ConnectionState::Connected(remote_addr),
                        ),
                    )));
            }
        } else {
            panic!("No address registered for a token which yielded a hello msg");
        }
    }

    fn handle_ack(&mut self, addr: &SocketAddr) -> () {
        if let Some(channel) = self.channel_map.get_mut(addr) {
            debug!(self.log, "Handling ack for {}", addr);
            channel.handle_ack();
            self.dispatcher_ref
                .tell(DispatchEnvelope::Event(EventEnvelope::Network(
                    NetworkEvent::Connection(*addr, ConnectionState::Connected(*addr)),
                )));
        }
    }

    fn try_write(&mut self, addr: &SocketAddr) -> IOReturn {
        if let Some(channel) = self.channel_map.get_mut(&addr) {
            match channel.try_drain() {
                Err(ref err) if broken_pipe(err) => {
                    return IOReturn::Close;
                }
                Ok(n) => {
                    self.sent_bytes += n as u64;
                }
                Err(e) => {
                    error!(
                        self.log,
                        "Unhandled error while writing to {}\n{:?}", addr, e
                    );
                }
            }
        }
        IOReturn::None
    }

    fn try_read(&mut self, addr: &SocketAddr) -> IOReturn {
        let mut ret = IOReturn::None;
        if let Some(channel) = self.channel_map.get_mut(&addr) {
            match channel.receive() {
                Ok(n) => {
                    self.received_bytes += n as u64;
                }
                Err(ref err) if no_buffer_space(err) => {
                    debug!(self.log, "no_buffer_space for channel {:?}", channel);
                    ret = IOReturn::SwapBuffer
                }
                Err(err) if interrupted(&err) || would_block(&err) => {
                    // Just retry later
                }
                Err(err) if connection_reset(&err) || broken_pipe(&err) => {
                    debug!(
                        self.log,
                        "Connection_reset to peer {}, shutting down the channel", &addr
                    );
                    ret = IOReturn::Close
                }
                Err(err) => {
                    // Fatal error don't try to read again
                    error!(
                        self.log,
                        "Error while reading from peer {}:\n{:?}", &addr, &err
                    );
                }
            }
        }
        ret
    }

    fn decode(&mut self, addr: &SocketAddr) -> IOReturn {
        let mut ret = IOReturn::None;
        // ret is used as return place-holder and internal flow-control.
        if let Some(channel) = self.channel_map.get_mut(addr) {
            loop {
                match channel.decode() {
                    Err(FramingError::NoData) => {
                        // Done
                        return ret;
                    }
                    Ok(Frame::Data(fr)) => {
                        use dispatch::lookup::ActorLookup;
                        use serialisation::ser_helpers::deserialise_msg;

                        // Forward the data frame to the correct actor
                        let lease_lookup = self.lookup.load();
                        let buf = fr.payload();
                        let envelope = deserialise_msg(buf).expect("s11n errors");
                        match lease_lookup.get_by_actor_path(&envelope.receiver) {
                            None => {
                                debug!(&self.log, "Could not find actor reference for destination: {:?}, dropping message", &envelope.receiver)
                            }
                            Some(actor) => {
                                actor.enqueue(envelope);
                            }
                        }
                    }
                    Ok(Frame::Hello(hello)) => {
                        // Channel handles hello internally. We can continue decoding.
                        debug!(self.log, "Handling Hello({}) from {}", &hello.addr, &addr);
                        channel.handle_hello(hello);
                    }
                    Ok(Frame::Start(start)) => {
                        // Channel handles hello internally. NetworkThread decides in next state transition
                        return IOReturn::Start(start.addr, start.id);
                    }
                    Ok(Frame::Ack(_)) => {
                        // We need to handle Acks immediately outside of the loop, then continue the loop
                        ret = IOReturn::Ack;
                        break;
                    }
                    Ok(Frame::Bye()) => {
                        debug!(self.log, "Received Bye from {}", &addr);
                        return IOReturn::Close;
                    }
                    Err(FramingError::InvalidMagicNum((check, slice))) => {
                        // There is no way to recover from this error right now. Would need resending mechanism
                        // or accept data loss and close the channel.
                        panic!("NetworkThread {} Unaligned buffer error for {}. {:?}, Magic_num: {:X}, Slice:{:?}",
                               self.addr, &addr, channel, check, slice);
                    }
                    Err(FramingError::InvalidFrame) => {
                        // Bad but not fatal error
                        error!(self.log, "Invalid Frame received on channel {:?}", channel);
                    }
                    Err(e) => {
                        error!(self.log, "Unhandled error {:?} from {:?}", &e, &addr);
                    }
                    Ok(other_frame) => error!(
                        self.log,
                        "Received unexpected frame type {:?} from {:?}",
                        other_frame.frame_type(),
                        channel
                    ),
                }
            }
        }
        match ret {
            IOReturn::Ack => {
                self.handle_ack(addr);
                // We must continue decoding after.
                self.decode(addr)
            }
            _ => ret,
        }
    }

    fn request_stream(&mut self, addr: SocketAddr) -> io::Result<()> {
        // Make sure we never request request a stream to someone we already have a connection to
        // Async communication with the dispatcher can lead to this
        if let Some(channel) = self.channel_map.remove(&addr) {
            // We already have a connection set-up
            // the connection request must have been sent before the channel was initialized
            if let ChannelState::Connected(_0, _1) = channel.state {
                // log and inform Dispatcher to make sure it knows we're connected.
                debug!(
                    self.log,
                    "Asked to request connection to already connected host {}", &addr
                );
                self.dispatcher_ref
                    .tell(DispatchEnvelope::Event(EventEnvelope::Network(
                        NetworkEvent::Connection(addr, ConnectionState::Connected(addr)),
                    )));
                self.channel_map.insert(addr, channel);
                return Ok(());
            } else {
                // It was an old attempt, remove it and continue with the new request
                drop(channel);
            }
        }
        debug!(self.log, "Requesting connection to {}", &addr);
        match TcpStream::connect(addr) {
            Ok(stream) => {
                self.store_stream(stream, &addr, ChannelState::Requested(addr, Uuid::new_v4()))?;
                Ok(())
            }
            Err(e) => {
                error!(
                    self.log,
                    "Failed to connect to remote host {}, error: {:?}", &addr, e
                );
                Ok(())
            }
        }
    }

    #[allow(irrefutable_let_patterns)]
    fn accept_stream(&mut self) -> io::Result<()> {
        while let (stream, addr) = (self.tcp_listener.as_ref().unwrap()).accept()? {
            debug!(self.log, "Accepting connection from {}", &addr);
            self.store_stream(stream, &addr, ChannelState::Initialising)?;
        }
        Ok(())
    }

    fn store_stream(
        &mut self,
        stream: TcpStream,
        addr: &SocketAddr,
        state: ChannelState,
    ) -> io::Result<()> {
        if let Some(buffer) = self.buffer_pool.get_buffer() {
            self.token_map.insert(self.token, *addr);
            let mut channel = TcpChannel::new(stream, self.token, buffer, state, self.addr);
            debug!(self.log, "Saying Hello to {}", addr);
            // Whatever error is thrown here will be re-triggered and handled later.
            channel.initialise(&self.addr);
            if let Err(e) = self.poll.registry().register(
                channel.stream_mut(),
                self.token,
                Interest::READABLE | Interest::WRITABLE,
            ) {
                error!(self.log, "Failed to register polling for {}\n{:?}", addr, e);
            }
            self.channel_map.insert(*addr, channel);
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
                DispatchEvent::SendTCP(addr, frame) => {
                    self.sent_msgs += 1;
                    // Get the token corresponding to the connection
                    if let Some(channel) = self.channel_map.get_mut(&addr) {
                        // The stream is already set-up, buffer the package and wait for writable event
                        if channel.connected() {
                            channel.enqueue_serialised(frame);
                        } else {
                            debug!(self.log, "Dispatch trying to route to non connected channel {:?}, rejecting the message", channel);
                            self.dispatcher_ref.tell(DispatchEnvelope::Event(
                                EventEnvelope::Network(NetworkEvent::RejectedFrame(addr, frame)),
                            ));
                            break;
                        }
                    } else {
                        // The stream isn't set-up, request connection, set-it up and try to send the message
                        debug!(self.log, "Dispatch trying to route to unrecognized address {}, rejecting the message", addr);
                        self.dispatcher_ref
                            .tell(DispatchEnvelope::Event(EventEnvelope::Network(
                                NetworkEvent::RejectedFrame(addr, frame),
                            )));
                        break;
                    }
                    if let IOReturn::Close = self.try_write(&addr) {
                        self.close_channel(addr);
                    }
                }
                DispatchEvent::SendUDP(addr, frame) => {
                    self.sent_msgs += 1;
                    // Get the token corresponding to the connection
                    if let Some(ref mut udp_state) = self.udp_state {
                        udp_state.enqueue_serialised(addr, frame);
                        match udp_state.try_write() {
                            Ok(n) => {
                                self.sent_bytes += n as u64;
                            }
                            Err(e) => {
                                warn!(self.log, "Error during UDP sending: {}", e);
                            }
                        }
                    } else {
                        warn!(
                            self.log,
                            "Rejecting UDP message to {} as socket is already shut down.", addr
                        );
                    }
                }
                DispatchEvent::Stop => {
                    self.stop();
                }
                DispatchEvent::Connect(addr) => {
                    debug!(self.log, "Got DispatchEvent::Connect({})", addr);
                    self.request_stream(addr)?;
                }
            }
        }
        Ok(())
    }

    fn close_channel(&mut self, addr: SocketAddr) -> () {
        if let Some(mut channel) = self.channel_map.remove(&addr) {
            self.dispatcher_ref
                .tell(DispatchEnvelope::Event(EventEnvelope::Network(
                    NetworkEvent::Connection(addr, ConnectionState::Closed),
                )));
            for rejected_frame in channel.take_outbound() {
                self.dispatcher_ref
                    .tell(DispatchEnvelope::Event(EventEnvelope::Network(
                        NetworkEvent::RejectedFrame(addr, rejected_frame),
                    )));
            }
            channel.shutdown();
            drop(channel);
        }
    }

    fn stop(&mut self) -> () {
        let tokens = self.token_map.clone();
        for (_, addr) in tokens {
            self.try_read(&addr);
        }
        for (_, mut channel) in self.channel_map.drain() {
            debug!(
                self.log,
                "Stopping channel with message count {}", channel.messages
            );
            channel.graceful_shutdown();
        }
        if let Some(mut listener) = self.tcp_listener.take() {
            self.poll.registry().deregister(&mut listener).ok();
            drop(listener);
            debug!(self.log, "Dropped its TCP server");
        }
        if let Some(mut udp_state) = self.udp_state.take() {
            self.poll.registry().deregister(&mut udp_state.socket).ok();
            let count = udp_state.pending_messages();
            drop(udp_state);
            debug!(
                self.log,
                "Dropped its UDP socket with message count {}", count
            );
        }
        self.stopped = true;
        debug!(self.log, "Stopped.");
    }

    fn next_token(&mut self) -> () {
        let next = self.token.0 + 1;
        self.token = Token(next);
    }
}

fn bind_with_retries(
    addr: &SocketAddr,
    retries: usize,
    log: &KompactLogger,
) -> io::Result<TcpListener> {
    match TcpListener::bind(*addr) {
        Ok(listener) => Ok(listener),
        Err(e) => {
            if retries > 0 {
                debug!(
                    log,
                    "Failed to bind to addr {}, will retry {} more times, error was: {:?}",
                    addr,
                    retries,
                    e
                );
                // Lets give cleanup some time to do it's thing before we retry
                thread::sleep(Duration::from_millis(BIND_RETRY_INTERVAL));
                bind_with_retries(addr, retries - 1, log)
            } else {
                Err(e)
            }
        }
    }
}

#[cfg(test)]
#[allow(unused_must_use)]
mod tests {
    use super::*;
    use crate::{dispatch::NetworkConfig, prelude::BufferConfig};
    use std::net::{IpAddr, Ipv4Addr};

    // Cleaner test-cases for manually running the thread
    fn poll_and_handle(thread: &mut NetworkThread) -> () {
        let mut events = Events::with_capacity(10);
        thread
            .poll
            .poll(&mut events, Some(Duration::from_millis(100)));
        for event in events.iter() {
            thread.handle_event(event);
        }
    }

    #[allow(unused_must_use)]
    fn setup_two_threads(
        addr1: SocketAddr,
        addr2: SocketAddr,
    ) -> (
        NetworkThread,
        Sender<DispatchEvent>,
        NetworkThread,
        Sender<DispatchEvent>,
    ) {
        let mut cfg = KompactConfig::new();
        cfg.system_components(DeadletterBox::new, NetworkConfig::default().build());
        let system = cfg.build().expect("KompactSystem");

        // Set-up the the threads arguments
        let lookup = Arc::new(ArcSwap::from_pointee(ActorStore::new()));
        //network_thread_registration.set_readiness(Interest::empty());
        let (input_queue_1_sender, input_queue_1_receiver) = channel();
        let (input_queue_2_sender, input_queue_2_receiver) = channel();
        let (dispatch_shutdown_sender1, _) = promise();
        let (dispatch_shutdown_sender2, _) = promise();
        let logger = system.logger().clone();
        let dispatcher_ref = system.dispatcher_ref();

        // Set up the two network threads
        let (network_thread1, _) = NetworkThread::new(
            logger.clone(),
            addr1,
            lookup.clone(),
            input_queue_1_receiver,
            dispatch_shutdown_sender1,
            dispatcher_ref.clone(),
            NetworkConfig::default(),
        );

        let (network_thread2, _) = NetworkThread::new(
            logger,
            addr2,
            lookup,
            input_queue_2_receiver,
            dispatch_shutdown_sender2,
            dispatcher_ref,
            NetworkConfig::default(),
        );

        (
            network_thread1,
            input_queue_1_sender,
            network_thread2,
            input_queue_2_sender,
        )
    }

    #[test]
    fn merge_connections_basic() -> () {
        // Sets up two NetworkThreads and does mutual connection request

        let addr1 = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 7778);
        let addr2 = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 7780);

        let (mut network_thread1, input_queue_1_sender, mut network_thread2, input_queue_2_sender) =
            setup_two_threads(addr1, addr2);

        // Tell both to connect to each-other before they start running:
        input_queue_1_sender.send(DispatchEvent::Connect(addr2));
        input_queue_2_sender.send(DispatchEvent::Connect(addr1));

        // Let both handle the connect event:
        network_thread1.receive_dispatch();
        network_thread2.receive_dispatch();

        // Wait for the connect requests to reach destination:
        thread::sleep(Duration::from_millis(100));

        // Accept requested streams
        network_thread1.accept_stream();
        network_thread2.accept_stream();

        // Wait for Hello to reach destination:
        thread::sleep(Duration::from_millis(100));

        // We need to make sure the TCP buffers are actually flushing the messages.
        // Handle events on both ends, say hello:
        poll_and_handle(&mut network_thread1);
        poll_and_handle(&mut network_thread2);
        thread::sleep(Duration::from_millis(100));
        // Cycle two Requested channels
        poll_and_handle(&mut network_thread1);
        poll_and_handle(&mut network_thread2);
        thread::sleep(Duration::from_millis(100));
        // Cycle three, merge and close
        poll_and_handle(&mut network_thread1);
        poll_and_handle(&mut network_thread2);
        thread::sleep(Duration::from_millis(100));
        // Cycle four, receive close and close
        poll_and_handle(&mut network_thread1);
        poll_and_handle(&mut network_thread2);
        thread::sleep(Duration::from_millis(100));
        // Now we can inspect the Network channels, both only have one channel:
        assert_eq!(network_thread1.channel_map.len(), 1);
        assert_eq!(network_thread2.channel_map.len(), 1);

        // Now assert that they've kept the same channel:
        assert_eq!(
            network_thread1
                .channel_map
                .drain()
                .next()
                .unwrap()
                .1
                .stream()
                .local_addr()
                .unwrap(),
            network_thread2
                .channel_map
                .drain()
                .next()
                .unwrap()
                .1
                .stream()
                .peer_addr()
                .unwrap()
        );
    }

    #[test]
    fn merge_connections_tricky() -> () {
        // Sets up two NetworkThreads and does mutual connection request
        // This test uses a different order of events than basic

        let addr1 = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8778);
        let addr2 = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8780);

        let (mut network_thread1, input_queue_1_sender, mut network_thread2, input_queue_2_sender) =
            setup_two_threads(addr1, addr2);

        // 2 Requests connection to 1 and sends Hello
        input_queue_2_sender.send(DispatchEvent::Connect(addr1));
        network_thread2.receive_dispatch();
        thread::sleep(Duration::from_millis(100));

        // 1 accepts the connection and sends hello back
        network_thread1.accept_stream();
        thread::sleep(Duration::from_millis(100));
        // 2 receives the Hello
        poll_and_handle(&mut network_thread2);
        thread::sleep(Duration::from_millis(100));
        // 1 Receives Hello
        poll_and_handle(&mut network_thread1);

        // 1 Receives Request Connection Event, this is the tricky part
        // 1 Requests connection to 2 and sends Hello
        input_queue_1_sender.send(DispatchEvent::Connect(addr2));
        network_thread1.receive_dispatch();
        thread::sleep(Duration::from_millis(100));

        // 2 accepts the connection and replies with hello
        network_thread2.accept_stream();
        thread::sleep(Duration::from_millis(100));

        // 2 receives the Hello on the new channel and merges
        poll_and_handle(&mut network_thread2);
        thread::sleep(Duration::from_millis(100));

        // 1 receives the Hello on the new channel and merges
        poll_and_handle(&mut network_thread1);
        thread::sleep(Duration::from_millis(100));

        // 2 receives the Bye and the Ack.
        poll_and_handle(&mut network_thread2);
        thread::sleep(Duration::from_millis(100));

        // Now we can inspect the Network channels, both only have one channel:
        assert_eq!(network_thread1.channel_map.len(), 1);
        assert_eq!(network_thread2.channel_map.len(), 1);

        // Now assert that they've kept the same channel:
        assert_eq!(
            network_thread1
                .channel_map
                .drain()
                .next()
                .unwrap()
                .1
                .stream()
                .local_addr()
                .unwrap(),
            network_thread2
                .channel_map
                .drain()
                .next()
                .unwrap()
                .1
                .stream()
                .peer_addr()
                .unwrap()
        );
    }

    #[test]
    fn network_thread_custom_buffer_config() -> () {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 9788);
        let buffer_config = BufferConfig::new(128, 13, 14, 10);
        let network_config = NetworkConfig::with_buffer_config(addr, buffer_config);
        let mut cfg = KompactConfig::new();
        cfg.system_components(DeadletterBox::new, NetworkConfig::default().build());
        let system = cfg.build().expect("KompactSystem");

        // Set-up the the threads arguments
        // TODO: Mock this properly instead
        let lookup = Arc::new(ArcSwap::from_pointee(ActorStore::new()));
        //network_thread_registration.set_readiness(Interest::empty());
        let (_, input_queue_1_receiver) = channel();
        let (dispatch_shutdown_sender1, _) = promise();
        let logger = system.logger().clone();
        let dispatcher_ref = system.dispatcher_ref();

        // Set up the two network threads
        let (mut network_thread, _) = NetworkThread::new(
            logger.clone(),
            addr,
            lookup.clone(),
            input_queue_1_receiver,
            dispatch_shutdown_sender1,
            dispatcher_ref.clone(),
            network_config,
        );
        // Assert that the buffer_pool is created correctly
        let (pool_size, _) = network_thread.buffer_pool.get_pool_sizes();
        assert_eq!(pool_size, 13); // initial_pool_size
        assert_eq!(network_thread.buffer_pool.get_buffer().unwrap().len(), 128);
        network_thread.stop();
    }

    /*
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
        network_thread2.poll.poll(&mut events, None);
        for event in events.iter() {
            network_thread2.handle_event(event);
        }
        events.clear();
    }*/
}

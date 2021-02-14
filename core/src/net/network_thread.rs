use super::*;
use crate::{
    dispatch::NetworkConfig,
    messaging::{DispatchEnvelope, EventEnvelope},
    net::{
        buffers::{BufferChunk, BufferPool, EncodeBuffer},
        network_channel::{ChannelState, TcpChannel},
        udp_state::UdpState,
        ConnectionState,
    },
};
use crossbeam_channel::Receiver as Recv;
use mio::{
    net::{TcpListener, TcpStream, UdpSocket},
    Events,
    Poll,
    Token,
};
use rustc_hash::FxHashMap;
use std::{
    collections::VecDeque,
    io,
    net::{Shutdown, SocketAddr},
    sync::Arc,
    time::Duration,
    usize,
};
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
    network_config: NetworkConfig,
    retry_queue: VecDeque<(Token, bool, bool, usize)>,
    out_of_buffers: bool,
    encode_buffer: EncodeBuffer,
}

/// Return values for IO Operations on the [NetworkChannel](net::network_channel::NetworkChannel) abstraction
#[derive(Debug, PartialEq, Eq)]
pub(super) enum IoReturn {
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
        network_config: NetworkConfig,
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
                    &network_config.get_buffer_config(),
                    &network_config.get_custom_allocator(),
                );

                let encode_buffer = EncodeBuffer::with_config(
                    &network_config.get_buffer_config(),
                    &network_config.get_custom_allocator(),
                );

                let udp_buffer = buffer_pool
                    .get_buffer()
                    .expect("Could not get buffer for setting up UDP");

                let udp_state =
                    UdpState::new(udp_socket, udp_buffer, logger.clone(), &network_config);
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
                        network_config,
                        retry_queue: VecDeque::new(),
                        out_of_buffers: false,
                        encode_buffer,
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
        let mut retry_queue = VecDeque::<(Token, bool, bool, usize)>::new();
        let mut timeout;
        loop {
            // Retries happen for connection interrupts and buffer-swapping
            // Performed in the main loop to avoid recursion
            retry_queue.append(&mut self.retry_queue);
            timeout = if self.out_of_buffers {
                // Avoid looping retries when there are no Buffers
                Some(Duration::from_millis(
                    self.network_config.get_connection_retry_interval(),
                ))
            } else if retry_queue.is_empty() {
                None
            }
            // No need to timeout if there are no retries
            else {
                Some(Duration::from_secs(0))
            }; // Timeout immediately if retries are waiting

            self.poll
                .poll(&mut events, timeout)
                .expect("Error when calling Poll");

            for (token, readable, writeable, retries) in events
                .iter()
                .map(|e| (e.token(), e.is_readable(), e.is_writable(), 0_usize))
                .chain(retry_queue.drain(0..retry_queue.len()))
            {
                self.handle_event(token, readable, writeable, retries);

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

    fn handle_event(&mut self, token: Token, readable: bool, writeable: bool, retries: usize) {
        match token {
            TCP_SERVER => {
                // Received an event for the TCP server socket.Accept the connection.
                if let Err(e) = self.accept_stream() {
                    debug!(self.log, "Error while accepting stream {:?}", e);
                }
            }
            UDP_SOCKET => {
                if let Some(ref mut udp_state) = self.udp_state {
                    if writeable {
                        match udp_state.try_write() {
                            Ok(n) => {
                                self.sent_bytes += n as u64;
                            }
                            Err(e) => {
                                warn!(self.log, "Error during UDP sending: {}", e);
                            }
                        }
                    }
                    if readable {
                        match udp_state.try_read() {
                            Ok((n, ioret)) => {
                                if n > 0 {
                                    self.received_bytes += n as u64;
                                }
                                if IoReturn::SwapBuffer == ioret {
                                    if let Some(mut new_buffer) = self.buffer_pool.get_buffer() {
                                        udp_state.swap_buffer(&mut new_buffer);
                                        self.buffer_pool.return_buffer(new_buffer);
                                        debug!(self.log, "Swapped UDP buffer");
                                        self.out_of_buffers = false;
                                        // We do not count successful swaps in the retries, stay at the same count
                                        self.retry_queue
                                            .push_back((token, readable, writeable, retries));
                                    } else {
                                        error!(
                                            self.log,
                                            "Could not get UDP buffer, retries: {}", retries
                                        );
                                        self.out_of_buffers = true;
                                        self.retry_queue.push_back((
                                            token,
                                            readable,
                                            writeable,
                                            retries + 1,
                                        ));
                                    }
                                }
                            }
                            Err(e) => {
                                warn!(self.log, "Error during UDP reading: {}", e);
                            }
                        }
                        use dispatch::lookup::{ActorLookup, LookupResult};

                        // Forward the data frame to the correct actor
                        let lease_lookup = self.lookup.load();
                        for envelope in udp_state.incoming_messages.drain(..) {
                            match lease_lookup.get_by_actor_path(&envelope.receiver) {
                                LookupResult::Ref(actor) => {
                                    actor.enqueue(envelope);
                                }
                                LookupResult::Group(group) => {
                                    group.route(envelope, &self.log);
                                }
                                LookupResult::None => {
                                    debug!(self.log, "Could not find actor reference for destination: {:?}, dropping message", envelope.receiver);
                                }
                                LookupResult::Err(e) => {
                                    error!(
                                        self.log,
                                        "An error occurred during local actor lookup for destination: {:?}, dropping message. The error was: {}",
                                        envelope.receiver,
                                        e
                                    );
                                }
                            }
                        }
                    }
                } else {
                    debug!(self.log, "Poll triggered for removed UDP socket");
                }
            }
            DISPATCHER => {
                // Message available from Dispatcher, clear the poll readiness before receiving
                self.receive_dispatch();
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
                        return;
                    }
                };
                let mut swap_buffer = false;
                let mut close_channel = false;
                if writeable {
                    if let IoReturn::Close = self.try_write(&addr) {
                        // Remove and deregister
                        close_channel = true;
                    }
                }
                if readable {
                    match self.try_read(&addr) {
                        IoReturn::Close => {
                            // Remove and deregister
                            close_channel = true;
                        }
                        IoReturn::SwapBuffer => {
                            swap_buffer = true;
                        }
                        _ => {}
                    }

                    match self.decode(&addr) {
                        IoReturn::Start(remote_addr, id) => {
                            self.handle_start(token, remote_addr, id);
                        }
                        IoReturn::Close => {
                            // Remove and deregister
                            close_channel = true;
                        }
                        _ => (),
                    }
                    if swap_buffer {
                        // Buffer full, we swap it and register for poll again
                        if let Some(channel) = self.channel_map.get_mut(&addr) {
                            if let Some(mut new_buffer) = self.buffer_pool.get_buffer() {
                                self.out_of_buffers = false;
                                channel.swap_buffer(&mut new_buffer);
                                self.buffer_pool.return_buffer(new_buffer);
                                debug!(
                                    self.log,
                                    "Swapped buffer for {:?}, \nbuffer_pool: {:?}",
                                    &channel,
                                    &self.buffer_pool
                                );
                                // We do not count successful swaps in the retries, stay at the same count
                                self.retry_queue
                                    .push_back((token, readable, writeable, retries));
                            } else if retries
                                <= self.network_config.get_max_connection_retry_attempts() as usize
                            {
                                error!(
                                    self.log,
                                    "Could not get buffer for channel {}, retries {}",
                                    &addr,
                                    retries
                                );
                                self.out_of_buffers = true;
                                self.retry_queue.push_back((
                                    token,
                                    readable,
                                    writeable,
                                    retries + 1,
                                ));
                            } else {
                                error!(
                                    self.log,
                                    "Giving up on reading channel {}, retries {},\
                                waiting for buffers timed out",
                                    &addr,
                                    retries
                                );
                                close_channel = true;
                            }
                        }
                        // We retry the event such that the read is performed with the new buffer.
                    }
                    if close_channel {
                        self.close_channel(addr);
                    }
                }
            }
        }
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
                // The channel we received the start on was already registered with the appropriate address.
                // There is no need to change anything, we can simply transition the channel.
                debug!(
                    self.log,
                    "Got Start({}, ...) from {}, already registered with correct addr",
                    &remote_addr,
                    &registered_addr
                );
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

                            if other_channel.connected() || other_id > id || other_channel.closed()
                            {
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
            panic!(
                "No address registered for a token which yielded a hello msg, \
            should not be possible"
            );
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

    fn try_write(&mut self, addr: &SocketAddr) -> IoReturn {
        if let Some(channel) = self.channel_map.get_mut(&addr) {
            match channel.try_drain() {
                Err(ref err) if broken_pipe(err) => {
                    return IoReturn::Close;
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
        IoReturn::None
    }

    fn try_read(&mut self, addr: &SocketAddr) -> IoReturn {
        let mut ret = IoReturn::None;
        if let Some(channel) = self.channel_map.get_mut(&addr) {
            match channel.receive() {
                Ok(n) => {
                    self.received_bytes += n as u64;
                }
                Err(ref err) if no_buffer_space(err) => {
                    debug!(self.log, "no_buffer_space for channel {:?}", channel);
                    ret = IoReturn::SwapBuffer
                }
                Err(err) if interrupted(&err) || would_block(&err) => {
                    // Just retry later
                }
                Err(err) if connection_reset(&err) || broken_pipe(&err) => {
                    debug!(
                        self.log,
                        "Connection_reset to peer {}, shutting down the channel", &addr
                    );
                    ret = IoReturn::Close
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

    fn decode(&mut self, addr: &SocketAddr) -> IoReturn {
        let mut ret = IoReturn::None;
        // ret is used as return place-holder and internal flow-control.
        if let Some(channel) = self.channel_map.get_mut(addr) {
            loop {
                match channel.decode() {
                    Err(FramingError::NoData) => {
                        // Done
                        return ret;
                    }
                    Ok(Frame::Data(fr)) => {
                        use dispatch::lookup::{ActorLookup, LookupResult};
                        use serialisation::ser_helpers::deserialise_chunk_lease;

                        // Forward the data frame to the correct actor
                        let lease_lookup = self.lookup.load();
                        let buf = fr.payload();
                        let envelope = deserialise_chunk_lease(buf).expect("s11n errors");
                        match lease_lookup.get_by_actor_path(&envelope.receiver) {
                            LookupResult::Ref(actor) => {
                                actor.enqueue(envelope);
                            }
                            LookupResult::Group(group) => {
                                group.route(envelope, &self.log);
                            }
                            LookupResult::None => {
                                warn!(self.log, "Could not find actor reference for destination: {:?}, dropping message", envelope.receiver);
                            }
                            LookupResult::Err(e) => {
                                error!(
                                    self.log,
                                    "An error occurred during local actor lookup for destination: {:?}, dropping message. The error was: {}",
                                    envelope.receiver,
                                    e
                                );
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
                        return IoReturn::Start(start.addr, start.id);
                    }
                    Ok(Frame::Ack(_)) => {
                        // We need to handle Acks immediately outside of the loop, then continue the loop
                        ret = IoReturn::Ack;
                        break;
                    }
                    Ok(Frame::Bye()) => {
                        debug!(self.log, "Received Bye from {}", &addr);
                        return IoReturn::Close;
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
            IoReturn::Ack => {
                self.handle_ack(addr);
                // We must continue decoding after.
                self.decode(addr)
            }
            _ => ret,
        }
    }

    fn request_stream(&mut self, addr: SocketAddr) {
        // Make sure we never request request a stream to someone we already have a connection to
        // Async communication with the dispatcher can lead to this
        if let Some(channel) = self.channel_map.remove(&addr) {
            // We already have a connection set-up
            // the connection request must have been sent before the channel was initialized
            match channel.state {
                ChannelState::Connected(_, _) => {
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
                    return;
                }
                ChannelState::Closed(_, _) => {
                    // We're waiting for the ClosedAck from the NetworkDispatcher
                    // This shouldn't happen but the system will likely recover from it eventually
                    debug!(
                        self.log,
                        "Requested connection to host before receiving ClosedAck {}", &addr
                    );
                    self.channel_map.insert(addr, channel);
                    return;
                }
                _ => {
                    // It was an old attempt, remove it and continue with the new request
                    drop(channel);
                }
            }
        }
        // Fetch a buffer before we make the request
        if let Some(buffer) = self.buffer_pool.get_buffer() {
            debug!(self.log, "Requesting connection to {}", &addr);
            match TcpStream::connect(addr) {
                Ok(stream) => {
                    self.store_stream(
                        stream,
                        &addr,
                        ChannelState::Requested(addr, Uuid::new_v4()),
                        buffer,
                    );
                }
                Err(e) => {
                    error!(
                        self.log,
                        "Failed to connect to remote host {}, error: {:?}", &addr, e
                    );
                }
            }
        } else {
            // No buffers available, we reject the connection
            self.dispatcher_ref
                .tell(DispatchEnvelope::Event(EventEnvelope::Network(
                    NetworkEvent::Connection(addr, ConnectionState::Closed),
                )));
        }
    }

    #[allow(irrefutable_let_patterns)]
    fn accept_stream(&mut self) -> io::Result<()> {
        while let (stream, addr) = (self.tcp_listener.as_ref().unwrap()).accept()? {
            if let Some(buffer) = self.buffer_pool.get_buffer() {
                debug!(self.log, "Accepting connection from {}", &addr);
                self.store_stream(stream, &addr, ChannelState::Initialising, buffer);
            } else {
                // If we can't get a buffer we reject the channel immediately
                stream.shutdown(Shutdown::Both)?;
            }
        }
        Ok(())
    }

    fn store_stream(
        &mut self,
        stream: TcpStream,
        addr: &SocketAddr,
        state: ChannelState,
        buffer: BufferChunk,
    ) {
        self.token_map.insert(self.token, *addr);
        let mut channel = TcpChannel::new(
            stream,
            self.token,
            buffer,
            state,
            self.addr,
            &self.network_config,
        );
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
    }

    fn receive_dispatch(&mut self) {
        while let Ok(event) = self.input_queue.try_recv() {
            match event {
                DispatchEvent::SendTcp(addr, data) => {
                    self.sent_msgs += 1;
                    // Get the token corresponding to the connection
                    if let Some(channel) = self.channel_map.get_mut(&addr) {
                        // The stream is already set-up, buffer the package and wait for writable event
                        if channel.connected() {
                            match data {
                                DispatchData::Serialised(frame) => {
                                    channel.enqueue_serialised(frame);
                                }
                                _ => {
                                    if let Err(e) = self
                                        .encode_buffer
                                        .get_buffer_encoder()
                                        .and_then(|mut buf| {
                                            channel.enqueue_serialised(
                                                data.into_serialised(&mut buf)?,
                                            );
                                            Ok(())
                                        })
                                    {
                                        warn!(self.log, "Error serialising message: {}", e);
                                    }
                                }
                            }
                        } else {
                            debug!(self.log, "Dispatch trying to route to non connected channel {:?}, rejecting the message", channel);
                            self.dispatcher_ref.tell(DispatchEnvelope::Event(
                                EventEnvelope::Network(NetworkEvent::RejectedData(addr, data)),
                            ));
                            break;
                        }
                    } else {
                        // The stream isn't set-up, request connection, set-it up and try to send the message
                        debug!(self.log, "Dispatch trying to route to unrecognized address {}, rejecting the message", addr);
                        self.dispatcher_ref
                            .tell(DispatchEnvelope::Event(EventEnvelope::Network(
                                NetworkEvent::RejectedData(addr, data),
                            )));
                        break;
                    }
                    if let IoReturn::Close = self.try_write(&addr) {
                        self.close_channel(addr);
                    }
                }
                DispatchEvent::SendUdp(addr, data) => {
                    self.sent_msgs += 1;
                    // Get the token corresponding to the connection
                    if let Some(ref mut udp_state) = self.udp_state {
                        match data {
                            DispatchData::Serialised(frame) => {
                                udp_state.enqueue_serialised(addr, frame);
                            }
                            _ => {
                                if let Err(e) =
                                    self.encode_buffer.get_buffer_encoder().and_then(|mut buf| {
                                        udp_state.enqueue_serialised(
                                            addr,
                                            data.into_serialised(&mut buf)?,
                                        );
                                        Ok(())
                                    })
                                {
                                    warn!(self.log, "Error serialising message: {}", e);
                                }
                            }
                        }
                        match udp_state.try_write() {
                            Ok(n) => {
                                self.sent_bytes += n as u64;
                            }
                            Err(e) => {
                                warn!(self.log, "Error during UDP sending: {}", e);
                                debug!(self.log, "UDP erro debug info: {:?}", e);
                            }
                        }
                    } else {
                        self.dispatcher_ref
                            .tell(DispatchEnvelope::Event(EventEnvelope::Network(
                                NetworkEvent::RejectedData(addr, data),
                            )));
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
                    self.request_stream(addr);
                }
                DispatchEvent::ClosedAck(addr) => {
                    debug!(self.log, "Got DispatchEvent::ClosedAck({})", addr);
                    self.handle_closed_ack(addr);
                }
            }
        }
    }

    fn close_channel(&mut self, addr: SocketAddr) -> () {
        // We will only drop the Channel once we get the CloseAck from the NetworkDispatcher
        // Which ensures that the
        if let Some(channel) = self.channel_map.get_mut(&addr) {
            self.dispatcher_ref
                .tell(DispatchEnvelope::Event(EventEnvelope::Network(
                    NetworkEvent::Connection(addr, ConnectionState::Closed),
                )));
            for rejected_frame in channel.take_outbound() {
                self.dispatcher_ref
                    .tell(DispatchEnvelope::Event(EventEnvelope::Network(
                        NetworkEvent::RejectedData(addr, DispatchData::Serialised(rejected_frame)),
                    )));
            }
            channel.shutdown();
        }
    }

    fn handle_closed_ack(&mut self, addr: SocketAddr) -> () {
        if let Some(channel) = self.channel_map.remove(&addr) {
            match channel.state {
                ChannelState::Connected(_, _) => {
                    error!(self.log, "ClosedAck for connected Channel: {:#?}", channel);
                    self.channel_map.insert(addr, channel);
                }
                _ => {
                    let buffer = channel.destroy();
                    self.buffer_pool.return_buffer(buffer);
                }
            }
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
    use crate::{dispatch::NetworkConfig, net::buffers::BufferConfig};

    // Cleaner test-cases for manually running the thread
    fn poll_and_handle(thread: &mut NetworkThread) -> () {
        let mut events = Events::with_capacity(10);
        thread
            .poll
            .poll(&mut events, Some(Duration::from_millis(100)));
        for event in events.iter() {
            thread.handle_event(event.token(), event.is_readable(), event.is_writable(), 0);
        }
    }

    #[allow(unused_must_use)]
    fn setup_two_threads() -> (
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
            "127.0.0.1:0".parse().expect("Address should work"),
            lookup.clone(),
            input_queue_1_receiver,
            dispatch_shutdown_sender1,
            dispatcher_ref.clone(),
            NetworkConfig::default(),
        );

        let (network_thread2, _) = NetworkThread::new(
            logger,
            "127.0.0.1:0".parse().expect("Address should work"),
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
        let (mut thread1, input_queue_1_sender, mut thread2, input_queue_2_sender) =
            setup_two_threads();
        let addr1 = thread1.addr;
        let addr2 = thread2.addr;
        // Tell both to connect to each-other before they start running:
        input_queue_1_sender.send(DispatchEvent::Connect(addr2));
        input_queue_2_sender.send(DispatchEvent::Connect(addr1));

        // Let both handle the connect event:
        thread1.receive_dispatch();
        thread2.receive_dispatch();

        // Wait for the connect requests to reach destination:
        thread::sleep(Duration::from_millis(100));

        // Accept requested streams
        thread1.accept_stream();
        thread2.accept_stream();

        // Wait for Hello to reach destination:
        thread::sleep(Duration::from_millis(100));

        // We need to make sure the TCP buffers are actually flushing the messages.
        // Handle events on both ends, say hello:
        poll_and_handle(&mut thread1);
        poll_and_handle(&mut thread2);
        thread::sleep(Duration::from_millis(100));
        // Cycle two Requested channels
        poll_and_handle(&mut thread1);
        poll_and_handle(&mut thread2);
        thread::sleep(Duration::from_millis(100));
        // Cycle three, merge and close
        poll_and_handle(&mut thread1);
        poll_and_handle(&mut thread2);
        thread::sleep(Duration::from_millis(100));
        // Cycle four, receive close and close
        poll_and_handle(&mut thread1);
        poll_and_handle(&mut thread2);
        thread::sleep(Duration::from_millis(100));
        // Now we can inspect the Network channels, both only have one channel:
        assert_eq!(thread1.channel_map.len(), 1);
        assert_eq!(thread2.channel_map.len(), 1);

        // Now assert that they've kept the same channel:
        assert_eq!(
            thread1
                .channel_map
                .drain()
                .next()
                .unwrap()
                .1
                .stream()
                .local_addr()
                .unwrap(),
            thread2
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
        let (mut thread1, input_queue_1_sender, mut thread2, input_queue_2_sender) =
            setup_two_threads();
        let addr1 = thread1.addr;
        let addr2 = thread2.addr;
        // 2 Requests connection to 1 and sends Hello
        input_queue_2_sender.send(DispatchEvent::Connect(addr1));
        thread2.receive_dispatch();
        thread::sleep(Duration::from_millis(100));

        // 1 accepts the connection and sends hello back
        thread1.accept_stream();
        thread::sleep(Duration::from_millis(100));
        // 2 receives the Hello
        poll_and_handle(&mut thread2);
        thread::sleep(Duration::from_millis(100));
        // 1 Receives Hello
        poll_and_handle(&mut thread1);

        // 1 Receives Request Connection Event, this is the tricky part
        // 1 Requests connection to 2 and sends Hello
        input_queue_1_sender.send(DispatchEvent::Connect(addr2));
        thread1.receive_dispatch();
        thread::sleep(Duration::from_millis(100));

        // 2 accepts the connection and replies with hello
        thread2.accept_stream();
        thread::sleep(Duration::from_millis(100));

        // 2 receives the Hello on the new channel and merges
        poll_and_handle(&mut thread2);
        thread::sleep(Duration::from_millis(100));

        // 1 receives the Hello on the new channel and merges
        poll_and_handle(&mut thread1);
        thread::sleep(Duration::from_millis(100));

        // 2 receives the Bye and the Ack.
        poll_and_handle(&mut thread2);
        thread::sleep(Duration::from_millis(100));

        // Now we can inspect the Network channels, both only have one channel:
        assert_eq!(thread1.channel_map.len(), 1);
        assert_eq!(thread2.channel_map.len(), 1);

        // Now assert that they've kept the same channel:
        assert_eq!(
            thread1
                .channel_map
                .drain()
                .next()
                .unwrap()
                .1
                .stream()
                .local_addr()
                .unwrap(),
            thread2
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
        let addr = "127.0.0.1:0".parse().expect("Address should work");
        let mut buffer_config = BufferConfig::default();
        buffer_config.chunk_size(128);
        buffer_config.max_chunk_count(14);
        buffer_config.initial_chunk_count(13);
        buffer_config.encode_buf_min_free_space(10);
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
            logger,
            addr,
            lookup,
            input_queue_1_receiver,
            dispatch_shutdown_sender1,
            dispatcher_ref,
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

use super::*;
use crate::{
    dispatch::{
        lookup::{ActorLookup, LookupResult},
        NetworkConfig,
    },
    messaging::{DispatchEnvelope, EventEnvelope, NetMessage, SerialisedFrame},
    net::{
        buffers::{BufferChunk, BufferPool, EncodeBuffer},
        network_channel::{ChannelState, TcpChannel},
        udp_state::UdpState,
    },
    prelude::{NetworkStatus, SessionId},
    serialisation::ser_helpers::deserialise_chunk_lease,
};
use crossbeam_channel::Receiver as Recv;
use lru::LruCache;
use mio::{
    event::Event,
    net::{TcpListener, TcpStream, UdpSocket},
    Events,
    Poll,
    Token,
};
use rustc_hash::{FxHashMap, FxHashSet};
use std::{
    cell::{RefCell, RefMut},
    collections::VecDeque,
    io,
    net::{IpAddr, Shutdown, SocketAddr},
    ops::DerefMut,
    rc::Rc,
    sync::Arc,
    time::Duration,
    usize,
};

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

/// Builder struct, can be sent to a thread safely to launch a NetworkThread
pub struct NetworkThreadBuilder {
    poll: Poll,
    waker: Option<Waker>,
    log: KompactLogger,
    pub address: SocketAddr,
    lookup: Arc<ArcSwap<ActorStore>>,
    input_queue: Recv<DispatchEvent>,
    shutdown_promise: KPromise<()>,
    dispatcher_ref: DispatcherRef,
    network_config: NetworkConfig,
    tcp_listener: TcpListener,
}

impl NetworkThreadBuilder {
    pub(crate) fn new(
        log: KompactLogger,
        address: SocketAddr,
        lookup: Arc<ArcSwap<ActorStore>>,
        input_queue: Recv<DispatchEvent>,
        shutdown_promise: KPromise<()>,
        dispatcher_ref: DispatcherRef,
        network_config: NetworkConfig,
    ) -> Result<NetworkThreadBuilder, NetworkBridgeErr> {
        let poll = Poll::new().expect("failed to create Poll instance in NetworkThread");
        let waker =
            Waker::new(poll.registry(), DISPATCHER).expect("failed to create Waker for DISPATCHER");
        let tcp_listener = bind_with_retries(&address, MAX_BIND_RETRIES, &log)?;
        let actual_address = tcp_listener.local_addr()?;
        Ok(NetworkThreadBuilder {
            poll,
            tcp_listener,
            waker: Some(waker),
            log,
            address: actual_address,
            lookup,
            input_queue,
            shutdown_promise,
            dispatcher_ref,
            network_config,
        })
    }

    pub fn take_waker(&mut self) -> Option<Waker> {
        self.waker.take()
    }

    pub fn build(mut self) -> NetworkThread {
        let actual_addr = self
            .tcp_listener
            .local_addr()
            .expect("could not get real addr");
        let logger = self.log.new(o!("addr" => format!("{}", actual_addr)));
        let mut udp_socket = UdpSocket::bind(actual_addr).expect("could not bind UDP on TCP port");

        // Register Listeners
        self.poll
            .registry()
            .register(&mut self.tcp_listener, TCP_SERVER, Interest::READABLE)
            .expect("failed to register TCP SERVER");
        self.poll
            .registry()
            .register(
                &mut udp_socket,
                UDP_SOCKET,
                Interest::READABLE | Interest::WRITABLE,
            )
            .expect("failed to register UDP SOCKET");

        let mut buffer_pool = BufferPool::with_config(
            self.network_config.get_buffer_config(),
            self.network_config.get_custom_allocator(),
        );
        let encode_buffer = EncodeBuffer::with_config(
            self.network_config.get_buffer_config(),
            self.network_config.get_custom_allocator(),
        );
        let udp_buffer = buffer_pool
            .get_buffer()
            .expect("Could not get buffer for setting up UDP");
        let udp_state = UdpState::new(udp_socket, udp_buffer, logger.clone(), &self.network_config);

        NetworkThread {
            log: logger,
            addr: actual_addr,
            lookup: self.lookup,
            tcp_listener: self.tcp_listener,
            udp_state: Some(udp_state),
            poll: self.poll,
            address_map: FxHashMap::default(),
            token_map: LruCache::new(self.network_config.get_hard_connection_limit() as usize),
            token: START_TOKEN,
            input_queue: self.input_queue,
            buffer_pool: RefCell::new(buffer_pool),
            stopped: false,
            shutdown_promise: self.shutdown_promise,
            dispatcher_ref: self.dispatcher_ref,
            network_config: self.network_config,
            retry_queue: VecDeque::new(),
            out_of_buffers: false,
            encode_buffer,
            block_list: AdressSet::default(), // TODO: extend NetworkConfig to build NetworkThread with a blocklist
        }
    }
}
/// Thread structure responsible for driving the Network IO
pub struct NetworkThread {
    log: KompactLogger,
    pub addr: SocketAddr,
    lookup: Arc<ArcSwap<ActorStore>>,
    tcp_listener: TcpListener,
    udp_state: Option<UdpState>,
    poll: Poll,
    address_map: FxHashMap<SocketAddr, Rc<RefCell<TcpChannel>>>,
    token_map: LruCache<Token, Rc<RefCell<TcpChannel>>>,
    token: Token,
    input_queue: Recv<DispatchEvent>,
    dispatcher_ref: DispatcherRef,
    buffer_pool: RefCell<BufferPool>,
    stopped: bool,
    shutdown_promise: KPromise<()>,
    network_config: NetworkConfig,
    retry_queue: VecDeque<EventWithRetries>,
    out_of_buffers: bool,
    encode_buffer: EncodeBuffer,
    block_list: AdressSet,
}

impl NetworkThread {
    pub fn run(mut self) -> () {
        trace!(self.log, "NetworkThread starting");
        let mut events = Events::with_capacity(MAX_POLL_EVENTS);
        loop {
            self.poll
                .poll(&mut events, self.get_poll_timeout())
                .expect("Error when calling Poll");

            for event in events
                .iter()
                .map(|e| (EventWithRetries::from(e)))
                .chain(self.retry_queue.split_off(0))
            {
                self.handle_event(event);

                if self.stopped {
                    if let Err(e) = self.shutdown_promise.complete() {
                        error!(self.log, "Error, shutting down sender: {:?}", e);
                    };
                    trace!(self.log, "Stopped");
                    return;
                };
            }
        }
    }

    fn get_poll_timeout(&self) -> Option<Duration> {
        if self.out_of_buffers {
            Some(Duration::from_millis(
                self.network_config.get_connection_retry_interval(),
            ))
        } else if self.retry_queue.is_empty() {
            None
        } else {
            Some(Duration::from_secs(0))
        }
    }

    fn handle_event(&mut self, event: EventWithRetries) {
        match event.token {
            TCP_SERVER => {
                if let Err(e) = self.receive_stream() {
                    error!(self.log, "Error while accepting stream {:?}", e);
                }
            }
            UDP_SOCKET => {
                if let Some(mut udp_state) = self.udp_state.take() {
                    if event.writeable {
                        self.write_udp(&mut udp_state);
                    }
                    if event.readable {
                        self.read_udp(&mut udp_state, event);
                    }
                    self.udp_state = Some(udp_state);
                }
            }
            DISPATCHER => {
                self.receive_dispatch();
            }
            _ => {
                if event.writeable {
                    self.write_tcp(&event.token);
                }
                if event.readable {
                    self.read_tcp(&event);
                }
            }
        }
    }

    fn retry_event(&mut self, event: &EventWithRetries) -> () {
        if event.retries <= self.network_config.get_max_connection_retry_attempts() {
            self.retry_queue.push_back(event.get_retry_event());
        } else if let Some(channel) = self.get_channel_by_token(&event.token) {
            self.lost_connection(channel.borrow_mut());
        }
    }

    fn enqueue_writeable_event(&mut self, token: &Token) -> () {
        self.retry_queue
            .push_back(EventWithRetries::writeable_with_token(token));
    }

    fn get_buffer(&self) -> Option<BufferChunk> {
        self.buffer_pool.borrow_mut().get_buffer()
    }

    fn return_buffer(&self, buffer: BufferChunk) -> () {
        self.buffer_pool.borrow_mut().return_buffer(buffer)
    }

    fn receive_dispatch(&mut self) {
        while let Ok(event) = self.input_queue.try_recv() {
            self.handle_dispatch_event(event);
        }
    }

    fn handle_dispatch_event(&mut self, event: DispatchEvent) {
        match event {
            DispatchEvent::SendTcp(address, data) => {
                self.send_tcp_message(address, data);
            }
            DispatchEvent::SendUdp(address, data) => {
                self.send_udp_message(address, data);
            }
            DispatchEvent::Stop => {
                self.stop();
            }
            DispatchEvent::Kill => {
                self.kill();
            }
            DispatchEvent::Connect(addr) => {
                if self.block_list.contains_ip_addr(&addr.ip())
                    || self.block_list.contains_socket_addr(&addr)
                {
                    return;
                }
                self.request_stream(addr);
            }
            DispatchEvent::ClosedAck(addr) => {
                self.handle_closed_ack(addr);
            }
            DispatchEvent::Close(addr) => {
                self.close_connection(addr);
            }
            DispatchEvent::BlockSocket(addr) => {
                self.block_socket_addr(addr, true);
            }
            DispatchEvent::BlockIpAddr(ip_addr) => {
                self.block_ip_addr(ip_addr);
            }
            DispatchEvent::UnblockSocket(addr) => {
                self.unblock_socket_addr(addr, true);
            }
            DispatchEvent::UnblockIpAddr(ip_addr) => {
                self.unblock_ip_addr(ip_addr);
            }
        }
    }

    fn get_channel_by_token(&mut self, token: &Token) -> Option<Rc<RefCell<TcpChannel>>> {
        self.token_map.get(token).cloned()
    }

    fn update_lru(&mut self, token: &Token) -> () {
        let _ = self.token_map.get(token);
    }

    fn get_channel_by_address(&self, address: &SocketAddr) -> Option<Rc<RefCell<TcpChannel>>> {
        self.address_map.get(address).cloned()
    }

    fn reregister_channel_address(
        &mut self,
        old_address: SocketAddr,
        new_address: SocketAddr,
    ) -> () {
        if let Some(channel_rc) = self.address_map.remove(&old_address) {
            self.address_map.insert(new_address, channel_rc);
        }
    }

    fn read_tcp(&mut self, event: &EventWithRetries) -> () {
        if let Some(channel_rc) = self.get_channel_by_token(&event.token) {
            let mut channel = channel_rc.borrow_mut();
            loop {
                match channel.read_frame(&self.buffer_pool) {
                    Ok(None) => {
                        return;
                    }
                    Ok(Some(Frame::Data(data))) => {
                        self.handle_data_frame(
                            data,
                            channel
                                .session_id()
                                .expect("Connected Channel must have a SessionId"),
                        );
                    }
                    Ok(Some(Frame::Start(start))) => {
                        self.handle_start(event, &mut channel, &start);
                        return;
                    }
                    Ok(Some(Frame::Hello(hello))) => {
                        self.handle_hello(&mut *channel, &hello);
                    }
                    Ok(Some(Frame::Ack())) => {
                        self.check_soft_connection_limit();
                        self.notify_network_status(NetworkStatus::ConnectionEstablished(
                            SystemPath::with_socket(Transport::Tcp, channel.address()),
                            channel.session_id().unwrap(),
                        ))
                    }
                    Ok(Some(Frame::Bye())) => {
                        self.handle_bye(&mut channel);
                        return;
                    }
                    Err(e) if no_buffer_space(&e) => {
                        self.out_of_buffers = true;
                        warn!(self.log, "Out of Buffers");
                        drop(channel);
                        self.retry_event(event);
                        return;
                    }
                    Err(e) if connection_reset(&e) => {
                        warn!(
                            self.log,
                            "Connection lost, reset by peer {}",
                            channel.address()
                        );
                        self.lost_connection(channel);
                        return;
                    }
                    Err(e) => {
                        warn!(
                            self.log,
                            "Error reading from channel {}: {}",
                            channel.address(),
                            &e
                        );
                        return;
                    }
                }
            }
        }
    }

    fn read_udp(&mut self, udp_state: &mut UdpState, event: EventWithRetries) -> () {
        match udp_state.try_read(&self.buffer_pool) {
            Ok(_) => {}
            Err(e) if no_buffer_space(&e) => {
                warn!(
                    self.log,
                    "Could not get UDP buffer, retries: {}", event.retries
                );
                self.out_of_buffers = true;
                self.retry_event(&event);
            }
            Err(e) => {
                warn!(self.log, "Error during UDP reading: {}", e);
            }
        }
        while let Some(net_message) = udp_state.incoming_messages.pop_front() {
            self.deliver_net_message(net_message);
        }
    }

    fn write_tcp(&mut self, token: &Token) -> () {
        if let Some(channel_rc) = self.get_channel_by_token(token) {
            let mut channel = channel_rc.borrow_mut();
            match channel.try_drain() {
                Err(ref err) if broken_pipe(err) => {
                    self.lost_connection(channel);
                }
                Ok(_) => {
                    if let ChannelState::CloseReceived(addr, id) = channel.state {
                        channel.state = ChannelState::Closed(addr, id);
                        debug!(self.log, "Connection to {} shutdown gracefully", &addr);
                        self.deregister_channel(&mut *channel);
                        self.notify_network_status(NetworkStatus::ConnectionClosed(
                            SystemPath::with_socket(Transport::Tcp, channel.address()),
                            id,
                        ));
                        self.reject_outbound_for_channel(&mut channel);
                    }
                }
                Err(e) => {
                    warn!(
                        self.log,
                        "Unhandled error while writing to {}\n{:?}",
                        channel.address(),
                        e
                    );
                }
            }
        }
    }

    fn write_udp(&mut self, udp_state: &mut UdpState) -> () {
        match udp_state.try_write() {
            Ok(_) => {}
            Err(e) => {
                warn!(self.log, "Error during UDP sending: {}", e);
            }
        }
    }

    fn send_tcp_message(&mut self, address: SocketAddr, data: DispatchData) {
        if let Some(channel_rc) = self.get_channel_by_address(&address) {
            let mut channel = channel_rc.borrow_mut();
            self.update_lru(&channel.token);
            if channel.connected() {
                match self.serialise_dispatch_data(data) {
                    Ok(frame) => {
                        channel.enqueue_serialised(frame);
                        self.enqueue_writeable_event(&channel.token);
                    }
                    Err(e) if out_of_buffers(&e) => {
                        self.out_of_buffers = true;
                        warn!(
                            self.log,
                            "No network buffers available, dropping outbound message.\
                        slow down message rate or increase buffer limits."
                        );
                    }
                    Err(e) => {
                        error!(self.log, "Error serialising message {}", e);
                    }
                }
            } else {
                trace!(
                    self.log,
                    "Dispatch trying to route to non connected channel {:?}, rejecting the message",
                    channel
                );
                self.reject_dispatch_data(address, data);
            }
        } else {
            trace!(
                self.log,
                "Dispatch trying to route to unrecognized address {}, rejecting the message",
                address
            );
            self.reject_dispatch_data(address, data);
        }
    }

    fn send_udp_message(&mut self, address: SocketAddr, data: DispatchData) {
        if let Some(mut udp_state) = self.udp_state.take() {
            match self.serialise_dispatch_data(data) {
                Ok(frame) => {
                    udp_state.enqueue_serialised(address, frame);
                    match udp_state.try_write() {
                        Ok(_) => {}
                        Err(e) => {
                            warn!(self.log, "Error during UDP sending: {}", e);
                            debug!(self.log, "UDP error debug info: {:?}", e);
                        }
                    }
                }
                Err(e) if out_of_buffers(&e) => {
                    self.out_of_buffers = true;
                    warn!(
                        self.log,
                        "No network buffers available, dropping outbound message.\
                        slow down message rate or increase buffer limits."
                    );
                }
                Err(e) => {
                    error!(self.log, "Error serialising message {}", e);
                }
            }
            self.udp_state = Some(udp_state);
        } else {
            self.reject_dispatch_data(address, data);
            trace!(
                self.log,
                "Rejecting UDP message to {} as socket is already shut down.",
                address
            );
        }
    }

    fn handle_data_frame(&self, data: Data, session: SessionId) -> () {
        let buf = data.payload();
        let mut envelope = deserialise_chunk_lease(buf).expect("s11n errors");
        envelope.set_session(session);
        self.deliver_net_message(envelope);
    }

    fn deliver_net_message(&self, envelope: NetMessage) -> () {
        let lease_lookup = self.lookup.load();
        match lease_lookup.get_by_actor_path(&envelope.receiver) {
            LookupResult::Ref(actor) => {
                actor.enqueue(envelope);
            }
            LookupResult::Group(group) => {
                group.route(envelope, &self.log);
            }
            LookupResult::None => {
                warn!(
                    self.log,
                    "Could not find actor reference for destination: {:?}, dropping message",
                    envelope.receiver
                );
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

    fn handle_hello(&mut self, channel: &mut TcpChannel, hello: &Hello) {
        if self.block_list.contains_socket_addr(&hello.addr) {
            self.drop_channel(channel);
        } else {
            self.reregister_channel_address(channel.address(), hello.addr);
            channel.handle_hello(hello);
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
    fn handle_start(&mut self, event: &EventWithRetries, channel: &mut TcpChannel, start: &Start) {
        if self.block_list.contains_ip_addr(&start.addr.ip())
            || self.block_list.contains_socket_addr(&start.addr)
        {
            self.drop_channel(channel);
            return;
        }
        if let Some(other_channel_rc) = self.get_channel_by_address(&start.addr) {
            debug!(
                self.log,
                "Merging channels for remote system {}", &start.addr
            );
            let mut other_channel = other_channel_rc.borrow_mut();
            match other_channel.read_state() {
                ChannelState::Connected(_, _) => {
                    self.drop_channel(channel);
                    return;
                }
                ChannelState::Requested(_, other_id) if other_id.0 > start.id.0 => {
                    self.drop_channel(channel);
                    return;
                }
                ChannelState::Initialised(_, other_id) if other_id.0 > start.id.0 => {
                    self.drop_channel(channel);
                    return;
                }
                _ => {
                    self.drop_channel(other_channel.deref_mut());
                }
            }
        }
        self.reregister_channel_address(channel.address(), start.addr);
        channel.handle_start(start);
        self.retry_event(event);
        self.check_soft_connection_limit();
        self.notify_network_status(NetworkStatus::ConnectionEstablished(
            SystemPath::with_socket(Transport::Tcp, start.addr),
            start.id,
        ));
    }

    fn handle_bye(&mut self, channel: &mut TcpChannel) -> () {
        match channel.state {
            ChannelState::Closed(addr, id) => {
                debug!(self.log, "Connection shutdown gracefully");
                self.deregister_channel(channel);
                self.notify_network_status(NetworkStatus::ConnectionClosed(
                    SystemPath::with_socket(Transport::Tcp, addr),
                    id,
                ));
                self.reject_outbound_for_channel(channel);
            }
            ChannelState::CloseReceived(_, _) => {}
            _ => {
                self.drop_channel(channel);
            }
        }
    }

    fn handle_closed_ack(&mut self, address: SocketAddr) -> () {
        if let Some(channel_rc) = self.get_channel_by_address(&address) {
            let mut channel = channel_rc.borrow_mut();
            if let ChannelState::Connected(_, _) = channel.state {
                error!(self.log, "ClosedAck for connected Channel: {:#?}", &channel);
            } else {
                self.drop_channel(&mut channel)
            }
        } else {
            error!(
                self.log,
                "ClosedAck for unrecognized address: {:#?}", &address
            );
        }
    }

    fn drop_channel(&mut self, channel: &mut TcpChannel) {
        self.deregister_channel(channel);
        self.address_map.remove(&channel.address());
        channel.shutdown();
        let mut buffer = BufferChunk::new(0);
        channel.swap_buffer(&mut buffer);
        self.return_buffer(buffer);
    }

    fn deregister_channel(&mut self, channel: &mut TcpChannel) {
        let _ = self.poll.registry().deregister(channel.stream_mut());
        self.token_map.pop(&channel.token);
    }

    fn request_stream(&mut self, address: SocketAddr) {
        if let Some(channel_rc) = self.get_channel_by_address(&address) {
            let mut channel = channel_rc.borrow_mut();
            match channel.state {
                ChannelState::Connected(_, _) => {
                    debug!(
                        self.log,
                        "Asked to request connection to already connected host {}", &address
                    );
                    return;
                }
                ChannelState::Closed(_, _) => {
                    debug!(
                        self.log,
                        "Requested connection to host before receiving ClosedAck {}", &address
                    );
                    return;
                }
                _ => {
                    self.drop_channel(&mut channel);
                }
            }
        }
        if self.check_hard_connection_limit() {
            warn!(
                self.log,
                "Hard Connection limit reached, rejecting request to connect to remote \
                host {}",
                &address
            );
            return;
        }
        if let Some(buffer) = self.get_buffer() {
            trace!(self.log, "Requesting connection to {}", &address);
            match TcpStream::connect(address) {
                Ok(stream) => {
                    self.store_stream(
                        stream,
                        address,
                        ChannelState::Requested(address, SessionId::new_unique()),
                        buffer,
                    );
                }
                Err(e) => {
                    //  Connection will be re-requested
                    trace!(
                        self.log,
                        "Failed to connect to remote host {}, error: {:?}",
                        &address,
                        e
                    );
                    self.return_buffer(buffer);
                }
            }
        } else {
            self.out_of_buffers = true;
            trace!(
                self.log,
                "No Buffers available when attempting to connect to remote host {}",
                &address
            );
        }
    }

    fn receive_stream(&mut self) -> io::Result<()> {
        while let Ok((stream, address)) = self.tcp_listener.accept() {
            if self.block_list.contains_ip_addr(&address.ip()) {
                trace!(
                    self.log,
                    "Rejecting connection request from blocked source: {}",
                    &address
                );
                stream.shutdown(Shutdown::Both)?;
            } else if self.check_hard_connection_limit() {
                warn!(
                    self.log,
                    "Hard Connection limit reached, rejecting incoming connection \
                request from {}",
                    &address
                );
                stream.shutdown(Shutdown::Both)?;
            } else if let Some(buffer) = self.get_buffer() {
                trace!(self.log, "Accepting connection from {}", &address);
                self.store_stream(stream, address, ChannelState::Initialising, buffer);
            } else {
                warn!(
                    self.log,
                    "Network Thread out of buffers, rejecting incoming connection \
                request from {}",
                    &address
                );
                stream.shutdown(Shutdown::Both)?;
            }
        }
        Ok(())
    }

    fn store_stream(
        &mut self,
        stream: TcpStream,
        address: SocketAddr,
        state: ChannelState,
        buffer: BufferChunk,
    ) {
        let mut channel = TcpChannel::new(
            stream,
            self.token,
            address,
            buffer,
            state,
            self.addr,
            &self.network_config,
        );
        channel.initialise(&self.addr);
        if let Err(e) = self.poll.registry().register(
            channel.stream_mut(),
            self.token,
            Interest::READABLE | Interest::WRITABLE,
        ) {
            error!(
                self.log,
                "Failed to register polling for {}\n{:?}", address, e
            );
        }
        let rc = Rc::new(RefCell::new(channel));
        self.address_map.insert(address, rc.clone());
        self.token_map.put(self.token, rc);
        self.next_token();
    }

    /// Checks the current active channel count and initiates a graceful shutdown of the LRU channel.
    fn check_soft_connection_limit(&mut self) -> () {
        let limit = self.network_config.get_soft_connection_limit() as usize;
        // First condition allows for early returns without doing the count
        if self.token_map.len() > limit && self.count_active_connections() > limit {
            // Find the LRU ACTIVE connection
            for (_, channel) in self.token_map.iter().rev() {
                if channel.borrow().connected() {
                    let addr = channel.borrow().address();
                    warn!(
                        self.log,
                        "Soft Connection Limit exceeded! limit = {}. Closing channel {:?}",
                        self.network_config.get_soft_connection_limit(),
                        &channel.borrow(),
                    );
                    self.notify_network_status(NetworkStatus::SoftConnectionLimitExceeded);
                    self.close_connection(addr);
                    return;
                }
            }
        }
    }

    /// Returns true if the limit is reached, and notifies the NetworkDispatcher that it is reached.
    fn check_hard_connection_limit(&self) -> bool {
        if self.token_map.len() >= self.network_config.get_hard_connection_limit() as usize {
            self.notify_network_status(NetworkStatus::HardConnectionLimitReached);
            true
        } else {
            false
        }
    }

    fn count_active_connections(&self) -> usize {
        self.token_map
            .iter()
            .filter(|(_, connection)| {
                if let Ok(con) = connection.try_borrow() {
                    con.connected()
                } else {
                    true
                } // If we fail to borrow the connection it's active.
            })
            .count()
    }

    /// Initiates a graceful closing sequence if the channel is connected or
    fn close_connection(&mut self, addr: SocketAddr) -> () {
        if let Some(channel) = self.get_channel_by_address(&addr) {
            let mut channel_mut = channel.borrow_mut();
            if channel_mut.connected() {
                channel_mut.initiate_graceful_shutdown();
                self.update_lru(&channel_mut.token);
            } else {
                self.drop_channel(&mut *channel_mut);
            }
        }
    }

    /// Handles all logic necessary to shutdown a channel for which the connection has been lost.
    fn lost_connection(&mut self, mut channel: RefMut<TcpChannel>) -> () {
        trace!(self.log, "Lost connection to address {}", channel.address());
        if let Some(id) = channel.session_id() {
            self.notify_network_status(NetworkStatus::ConnectionLost(
                SystemPath::with_socket(Transport::Tcp, channel.address()),
                id,
            ));
        }
        self.reject_outbound_for_channel(&mut channel);
        // Try to inform the other end that we're closing the channel
        let _ = channel.send_bye();
        self.deregister_channel(&mut *channel);
        channel.shutdown();
    }

    fn reject_outbound_for_channel(&mut self, channel: &mut TcpChannel) -> () {
        for rejected_frame in channel.take_outbound() {
            self.reject_dispatch_data(channel.address(), DispatchData::Serialised(rejected_frame));
        }
    }

    fn stop(&mut self) -> () {
        for (_, channel_rc) in self.address_map.drain() {
            let mut channel = channel_rc.borrow_mut();
            debug!(
                self.log,
                "Stopping channel with message count {}", channel.messages
            );
            let _ = channel.initiate_graceful_shutdown();
        }
        self.poll
            .registry()
            .deregister(&mut self.tcp_listener)
            .expect("Deregistering listener while stopping network should work");
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
    }

    fn kill(&mut self) -> () {
        trace!(self.log, "Killing the NetworkThread");
        for (_, channel_rc) in self.address_map.drain() {
            channel_rc.borrow_mut().kill();
        }
        self.stop();
    }

    fn notify_network_status(&self, status: NetworkStatus) {
        self.dispatcher_ref
            .tell(DispatchEnvelope::Event(EventEnvelope::Network(status)))
    }

    fn reject_dispatch_data(&self, address: SocketAddr, data: DispatchData) {
        self.dispatcher_ref
            .tell(DispatchEnvelope::Event(EventEnvelope::RejectedData((
                address,
                Box::new(data),
            ))));
    }

    fn next_token(&mut self) -> () {
        let next = self.token.0 + 1;
        self.token = Token(next);
    }

    fn serialise_dispatch_data(&mut self, data: DispatchData) -> Result<SerialisedFrame, SerError> {
        match data {
            DispatchData::Serialised(frame) => Ok(frame),
            _ => data.into_serialised(&mut self.encode_buffer.get_buffer_encoder()?),
        }
    }

    fn block_ip_addr(&mut self, ip_addr: IpAddr) {
        if self.block_list.insert_ip_addr(ip_addr) {
            debug!(self.log, "Blocking ip: {:?}", ip_addr);
            let trigger_status_port = false; // don't trigger NetworkStatusPort per blocked socket, one event for ip_addr is enough
            let block_sockets: Vec<SocketAddr> = self
                .address_map
                .keys()
                .filter(|socket_addr| socket_addr.ip() == ip_addr)
                .copied()
                .collect();
            for socket_addr in block_sockets {
                self.block_socket_addr(socket_addr, trigger_status_port);
            }
        }
        self.notify_network_status(NetworkStatus::BlockedIp(ip_addr));
    }

    fn block_socket_addr(&mut self, socket_addr: SocketAddr, trigger_status_port: bool) {
        if self.block_list.insert_socket_addr(socket_addr) {
            debug!(self.log, "Blocking socket: {:?}", socket_addr);
            if let Some(channel_rc) = self.get_channel_by_address(&socket_addr) {
                debug!(
                    self.log,
                    "Dropping channel to blocked socket: {:?}", socket_addr
                );
                let mut channel = channel_rc.borrow_mut();
                self.drop_channel(&mut channel);
            }
        }
        if trigger_status_port {
            self.notify_network_status(NetworkStatus::BlockedSystem(SystemPath::with_socket(
                Transport::Tcp,
                socket_addr,
            )));
        }
    }

    fn unblock_ip_addr(&mut self, ip_addr: IpAddr) {
        if self.block_list.remove_ip_addr(&ip_addr) {
            debug!(self.log, "Unblocking ip: {:?}", ip_addr);
            let trigger_status_port = false; // don't trigger NetworkStatusPort per unblocked socket, one event for ip_addr is enough
            let unblock_sockets = self.block_list.get_sockets_with_ip(ip_addr);
            for socket_addr in unblock_sockets {
                self.unblock_socket_addr(socket_addr, trigger_status_port);
            }
        }
        self.notify_network_status(NetworkStatus::UnblockedIp(ip_addr));
    }

    fn unblock_socket_addr(&mut self, socket_addr: SocketAddr, trigger_status_port: bool) {
        if self.block_list.remove_socket_addr(&socket_addr) {
            debug!(self.log, "Unblocking socket: {:?}", socket_addr);
            if trigger_status_port {
                self.notify_network_status(NetworkStatus::UnblockedSystem(
                    SystemPath::with_socket(Transport::Tcp, socket_addr),
                ));
            }
        }
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

#[derive(Clone)]
struct EventWithRetries {
    token: Token,
    readable: bool,
    writeable: bool,
    retries: u8,
}
impl EventWithRetries {
    fn from(event: &Event) -> EventWithRetries {
        EventWithRetries {
            token: event.token(),
            readable: event.is_readable(),
            writeable: event.is_writable(),
            retries: 0,
        }
    }

    fn writeable_with_token(token: &Token) -> EventWithRetries {
        EventWithRetries {
            token: *token,
            readable: false,
            writeable: true,
            retries: 0,
        }
    }

    fn get_retry_event(&self) -> EventWithRetries {
        EventWithRetries {
            token: self.token,
            readable: self.readable,
            writeable: self.writeable,
            retries: self.retries + 1,
        }
    }
}

#[derive(Default)]
pub struct AdressSet {
    ip_addr: FxHashSet<IpAddr>,
    socket_addr: FxHashSet<SocketAddr>,
}

impl AdressSet {
    fn insert_ip_addr(&mut self, ip_addr: IpAddr) -> bool {
        self.ip_addr.insert(ip_addr)
    }

    fn insert_socket_addr(&mut self, socket_addr: SocketAddr) -> bool {
        self.socket_addr.insert(socket_addr)
    }

    fn contains_ip_addr(&self, ip_addr: &IpAddr) -> bool {
        self.ip_addr.contains(ip_addr)
    }

    fn contains_socket_addr(&self, socket_addr: &SocketAddr) -> bool {
        self.socket_addr.contains(socket_addr)
    }

    fn remove_ip_addr(&mut self, ip_addr: &IpAddr) -> bool {
        self.ip_addr.remove(ip_addr)
    }

    fn remove_socket_addr(&mut self, socket_addr: &SocketAddr) -> bool {
        self.socket_addr.remove(socket_addr)
    }

    fn get_sockets_with_ip(&self, ip_addr: IpAddr) -> Vec<SocketAddr> {
        self.socket_addr
            .iter()
            .filter(|socket_addr| socket_addr.ip() == ip_addr)
            .copied()
            .collect()
    }
}

#[cfg(test)]
#[allow(unused_must_use)]
mod tests {
    use super::*;
    use crate::{dispatch::NetworkConfig, net::buffers::BufferConfig};
    use std::str::FromStr;

    // Cleaner test-cases for manually running the thread
    fn poll_and_handle(thread: &mut NetworkThread) -> () {
        let mut events = Events::with_capacity(10);
        thread
            .poll
            .poll(&mut events, Some(Duration::from_millis(100)));
        for event in events.iter() {
            thread.handle_event(EventWithRetries::from(event));
        }
        while let Some(event) = thread.retry_queue.pop_front() {
            thread.handle_event(event);
        }
    }

    #[allow(unused_must_use)]
    fn setup_network_thread(
        network_config: &NetworkConfig,
    ) -> (NetworkThread, Sender<DispatchEvent>) {
        let mut cfg = KompactConfig::default();
        cfg.system_components(DeadletterBox::new, network_config.clone().build());
        let system = cfg.build().expect("KompactSystem");

        // Set-up the the threads arguments
        let lookup = Arc::new(ArcSwap::from_pointee(ActorStore::new()));
        //network_thread_registration.set_readiness(Interest::empty());
        let (input_queue_sender, input_queue_receiver) = channel();
        let (dispatch_shutdown_sender, _) = promise();
        let logger = system.logger().clone();
        let dispatcher_ref = system.dispatcher_ref();

        let network_thread = NetworkThreadBuilder::new(
            logger,
            "127.0.0.1:0".parse().expect("Address should work"),
            lookup,
            input_queue_receiver,
            dispatch_shutdown_sender,
            dispatcher_ref,
            network_config.clone(),
        )
        .expect("Should work")
        .build();
        (network_thread, input_queue_sender)
    }

    fn run_handshake_sequence(requester: &mut NetworkThread, acceptor: &mut NetworkThread) {
        requester.receive_dispatch();
        thread::sleep(Duration::from_millis(100));
        poll_and_handle(acceptor);
        thread::sleep(Duration::from_millis(100));
        poll_and_handle(requester);
        thread::sleep(Duration::from_millis(100));
        poll_and_handle(acceptor);
        thread::sleep(Duration::from_millis(100));
        poll_and_handle(requester);
        thread::sleep(Duration::from_millis(100));
        poll_and_handle(acceptor);
        thread::sleep(Duration::from_millis(100));
    }

    const PATH: &str = "local://127.0.0.1:0/test_actor";

    fn empty_message() -> DispatchData {
        let path = ActorPath::from_str(PATH).expect("a proper path");
        DispatchData::Lazy(Box::new(()), path.clone(), path)
    }

    #[test]
    fn merge_connections_basic() -> () {
        // Sets up two NetworkThreads and does mutual connection request
        let (mut thread1, input_queue_1_sender) = setup_network_thread(&NetworkConfig::default());
        let (mut thread2, input_queue_2_sender) = setup_network_thread(&NetworkConfig::default());
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
        thread1.receive_stream();
        thread2.receive_stream();

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
        assert_eq!(thread1.address_map.len(), 1);
        assert_eq!(thread2.address_map.len(), 1);

        // Now assert that they've kept the same channel:
        assert_eq!(
            thread1
                .address_map
                .drain()
                .next()
                .unwrap()
                .1
                .borrow_mut()
                .stream()
                .local_addr()
                .unwrap(),
            thread2
                .address_map
                .drain()
                .next()
                .unwrap()
                .1
                .borrow_mut()
                .stream()
                .peer_addr()
                .unwrap()
        );
    }

    #[test]
    fn merge_connections_tricky() -> () {
        // Sets up two NetworkThreads and does mutual connection request
        // This test uses a different order of events than basic
        let (mut thread1, input_queue_1_sender) = setup_network_thread(&NetworkConfig::default());
        let (mut thread2, input_queue_2_sender) = setup_network_thread(&NetworkConfig::default());
        let addr1 = thread1.addr;
        let addr2 = thread2.addr;
        // 2 Requests connection to 1 and sends Hello
        input_queue_2_sender.send(DispatchEvent::Connect(addr1));
        thread2.receive_dispatch();
        thread::sleep(Duration::from_millis(100));

        // 1 accepts the connection and sends hello back
        thread1.receive_stream();
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
        thread2.receive_stream();
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

        poll_and_handle(&mut thread1);
        thread::sleep(Duration::from_millis(100));

        // Now we can inspect the Network channels, both only have one channel:
        assert_eq!(thread1.address_map.len(), 1);
        assert_eq!(thread2.address_map.len(), 1);

        // Now assert that they've kept the same channel:
        assert_eq!(
            thread1
                .address_map
                .drain()
                .next()
                .unwrap()
                .1
                .borrow_mut()
                .stream()
                .local_addr()
                .unwrap(),
            thread2
                .address_map
                .drain()
                .next()
                .unwrap()
                .1
                .borrow_mut()
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
        let (mut network_thread, _) = setup_network_thread(&network_config);
        // Assert that the buffer_pool is created correctly
        let (pool_size, _) = network_thread.buffer_pool.borrow_mut().get_pool_sizes();
        assert_eq!(pool_size, 13); // initial_pool_size
        assert_eq!(
            network_thread
                .buffer_pool
                .borrow_mut()
                .get_buffer()
                .unwrap()
                .len(),
            128
        );
        network_thread.stop();
    }

    // Creates 5 different network_threads, connects "1" to 2, 3, and 4 properly, then the 5th,
    // asserts that the 2nd is disconnected, sends something on 3rd
    // then the 2nd connects to 1, and we asserts that 4 is dropped by 1.
    #[test]
    fn soft_channel_limit() -> () {
        let mut network_config = NetworkConfig::default();
        network_config.set_soft_connection_limit(3);
        let (mut thread1, input_queue_1_sender) = setup_network_thread(&network_config);
        let (mut thread2, input_queue_2_sender) = setup_network_thread(&network_config);
        let (mut thread3, _) = setup_network_thread(&network_config);
        let (mut thread4, _) = setup_network_thread(&network_config);
        let (mut thread5, _) = setup_network_thread(&network_config);
        let addr1 = thread1.addr;
        let addr2 = thread2.addr;
        let addr3 = thread3.addr;
        let addr4 = thread4.addr;
        let addr5 = thread5.addr;

        input_queue_1_sender.send(DispatchEvent::Connect(addr2));
        input_queue_1_sender.send(DispatchEvent::Connect(addr3));
        input_queue_1_sender.send(DispatchEvent::Connect(addr4));

        // Run the handhsake sequences
        run_handshake_sequence(&mut thread1, &mut thread2);
        run_handshake_sequence(&mut thread1, &mut thread3);
        run_handshake_sequence(&mut thread1, &mut thread4);

        // Assert that 2, 3, 4 is connected to 1
        assert!(thread1
            .address_map
            .get(&addr2)
            .unwrap()
            .borrow_mut()
            .connected());
        assert!(thread1
            .address_map
            .get(&addr3)
            .unwrap()
            .borrow_mut()
            .connected());
        assert!(thread1
            .address_map
            .get(&addr4)
            .unwrap()
            .borrow_mut()
            .connected());

        // Send something to 3 and 4, to ensure that 2 is LRU
        input_queue_1_sender.send(DispatchEvent::SendTcp(addr3, empty_message()));
        input_queue_1_sender.send(DispatchEvent::SendTcp(addr4, empty_message()));
        thread1.receive_dispatch();
        poll_and_handle(&mut thread1);
        thread::sleep(Duration::from_millis(100));

        // Initiate connection to 5, execute handshake
        input_queue_1_sender.send(DispatchEvent::Connect(addr5));
        run_handshake_sequence(&mut thread1, &mut thread5);

        // Assert that 2 is no longer connected to 1
        assert!(!thread1
            .address_map
            .get(&addr2)
            .unwrap()
            .borrow_mut()
            .connected());

        // Receive and send bye
        poll_and_handle(&mut thread2);
        thread::sleep(Duration::from_millis(100));
        // Receive bye
        poll_and_handle(&mut thread1);
        thread::sleep(Duration::from_millis(100));
        input_queue_1_sender.send(DispatchEvent::ClosedAck(addr2));
        thread1.receive_dispatch();

        // Assert that 2 is dropped
        assert!(thread1.address_map.get(&addr2).is_none());

        // Ack the closed connection on 2
        input_queue_2_sender.send(DispatchEvent::ClosedAck(addr1));
        thread2.receive_dispatch();

        // Initiate new connection to 1 from 2 and execute handshake
        input_queue_2_sender.send(DispatchEvent::Connect(addr1));
        run_handshake_sequence(&mut thread2, &mut thread1);

        // Receive the incoming connection and assert that 3 (LRU) is no longer connected
        poll_and_handle(&mut thread1);
        assert!(!thread1
            .address_map
            .get(&addr3)
            .unwrap()
            .borrow_mut()
            .connected());

        thread1.stop();
        thread2.stop();
        thread3.stop();
        thread4.stop();
        thread5.stop();
    }

    // Creates 5 different network_threads, connects "1" to 2, 3, and 4 properly, then the 5th,
    // asserts that the 5th is refused.
    // Then attempts to connect 5 to 1 and asserts that it's refused again
    #[test]
    fn hard_channel_limit() -> () {
        let mut network_config = NetworkConfig::default();
        network_config.set_hard_connection_limit(3);
        let (mut thread1, input_queue_1_sender) = setup_network_thread(&network_config);
        let (mut thread2, _) = setup_network_thread(&network_config);
        let (mut thread3, _) = setup_network_thread(&network_config);
        let (mut thread4, _) = setup_network_thread(&network_config);
        let (mut thread5, input_queue_5_sender) = setup_network_thread(&network_config);
        let addr1 = thread1.addr;
        let addr2 = thread2.addr;
        let addr3 = thread3.addr;
        let addr4 = thread4.addr;
        let addr5 = thread5.addr;

        input_queue_1_sender.send(DispatchEvent::Connect(addr2));
        input_queue_1_sender.send(DispatchEvent::Connect(addr3));
        input_queue_1_sender.send(DispatchEvent::Connect(addr4));

        // Run the handhsake sequences
        run_handshake_sequence(&mut thread1, &mut thread2);
        run_handshake_sequence(&mut thread1, &mut thread3);
        run_handshake_sequence(&mut thread1, &mut thread4);

        // Assert that 2, 3, 4 is connected to 1
        assert!(thread1
            .address_map
            .get(&addr2)
            .unwrap()
            .borrow_mut()
            .connected());
        assert!(thread1
            .address_map
            .get(&addr3)
            .unwrap()
            .borrow_mut()
            .connected());
        assert!(thread1
            .address_map
            .get(&addr4)
            .unwrap()
            .borrow_mut()
            .connected());

        // Initiate connection to 5, execute handshake
        input_queue_1_sender.send(DispatchEvent::Connect(addr5));
        thread1.receive_dispatch();
        // That it was immediately discarded
        assert!(thread1.address_map.get(&addr5).is_none());

        // This should do nothing...
        run_handshake_sequence(&mut thread1, &mut thread5);

        // Assert channels are unchanged
        assert!(thread1.address_map.get(&addr5).is_none());
        assert!(thread1
            .address_map
            .get(&addr2)
            .unwrap()
            .borrow_mut()
            .connected());
        assert!(thread1
            .address_map
            .get(&addr3)
            .unwrap()
            .borrow_mut()
            .connected());
        assert!(thread1
            .address_map
            .get(&addr4)
            .unwrap()
            .borrow_mut()
            .connected());

        // Initiate new connection to 1 from 2 and execute handshake
        input_queue_5_sender.send(DispatchEvent::Connect(addr1));
        thread5.receive_dispatch();
        thread::sleep(Duration::from_millis(100));
        poll_and_handle(&mut thread1);
        // Should have been rejected immediately
        assert!(thread1.address_map.get(&addr5).is_none());

        // This should do nothing
        run_handshake_sequence(&mut thread5, &mut thread1);

        // Assert channels are unchanged
        assert!(thread1.address_map.get(&addr5).is_none());
        assert!(thread1
            .address_map
            .get(&addr2)
            .unwrap()
            .borrow_mut()
            .connected());
        assert!(thread1
            .address_map
            .get(&addr3)
            .unwrap()
            .borrow_mut()
            .connected());
        assert!(thread1
            .address_map
            .get(&addr4)
            .unwrap()
            .borrow_mut()
            .connected());

        thread1.stop();
        thread2.stop();
        thread3.stop();
        thread4.stop();
        thread5.stop();
    }
}

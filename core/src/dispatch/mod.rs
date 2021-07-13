use super::*;

use crate::{
    actors::{Actor, ActorPath, Dispatcher, DynActorRef, SystemPath, Transport},
    component::{Component, ComponentContext, ExecuteResult},
};
use std::{pin::Pin, sync::Arc};

use crate::{
    actors::{NamedPath, Transport::Tcp},
    messaging::{
        ActorRegistration,
        DispatchData,
        DispatchEnvelope,
        EventEnvelope,
        MsgEnvelope,
        NetMessage,
        PathResolvable,
        PolicyRegistration,
        RegistrationEnvelope,
        RegistrationError,
        RegistrationEvent,
        RegistrationPromise,
    },
    net::{buffers::*, ConnectionState, NetworkBridgeErr, Protocol, SocketAddr},
    prelude::SessionId,
    timer::timer_manager::Timer,
};
use arc_swap::ArcSwap;
use futures::{
    self,
    task::{Context, Poll},
};
use lookup::{ActorLookup, ActorStore, InsertResult, LookupResult};
use queue_manager::QueueManager;
use rustc_hash::FxHashMap;
use std::{collections::VecDeque, net::IpAddr, time::Duration};

pub mod lookup;
pub mod queue_manager;

// Default values for network config.
mod defaults {
    pub(crate) const RETRY_CONNECTIONS_INTERVAL: u64 = 5000;
    pub(crate) const BOOT_TIMEOUT: u64 = 5000;
    pub(crate) const MAX_RETRY_ATTEMPTS: u8 = 10;
    pub(crate) const SOFT_CONNECTION_LIMIT: u32 = 1000;
    pub(crate) const HARD_CONNECTION_LIMIT: u32 = 1100;
}

type NetHashMap<K, V> = FxHashMap<K, V>;

/// Configuration builder for the network dispatcher
///
/// # Example
///
/// This example binds to local host on a free port chosen by the operating system.
///
/// ```
/// use kompact::prelude::*;
///
/// let mut conf = KompactConfig::default();
/// conf.system_components(DeadletterBox::new, NetworkConfig::default().build());
/// let system = conf.build().expect("system");
/// # system.shutdown().expect("shutdown");
/// ```
#[derive(Clone, Debug)]
pub struct NetworkConfig {
    addr: SocketAddr,
    transport: Transport,
    buffer_config: BufferConfig,
    custom_allocator: Option<Arc<dyn ChunkAllocator>>,
    tcp_nodelay: bool,
    max_connection_retry_attempts: u8,
    connection_retry_interval: u64,
    boot_timeout: u64,
    soft_connection_limit: u32,
    hard_connection_limit: u32,
}

impl NetworkConfig {
    /// Create a new config with `addr` and protocol [TCP](Transport::Tcp)
    /// NetworkDispatcher and NetworkThread will use the default `BufferConfig`
    pub fn new(addr: SocketAddr) -> Self {
        NetworkConfig {
            addr,
            transport: Transport::Tcp,
            buffer_config: BufferConfig::default(),
            custom_allocator: None,
            tcp_nodelay: true,
            max_connection_retry_attempts: defaults::MAX_RETRY_ATTEMPTS,
            connection_retry_interval: defaults::RETRY_CONNECTIONS_INTERVAL,
            boot_timeout: defaults::BOOT_TIMEOUT,
            soft_connection_limit: defaults::SOFT_CONNECTION_LIMIT,
            hard_connection_limit: defaults::HARD_CONNECTION_LIMIT,
        }
    }

    /// Create a new config with `addr` and protocol [TCP](Transport::Tcp)
    /// Note: Only the NetworkThread and NetworkDispatcher will use the `BufferConfig`, not Actors
    pub fn with_buffer_config(addr: SocketAddr, buffer_config: BufferConfig) -> Self {
        buffer_config.validate();
        let mut cfg = NetworkConfig::new(addr);
        cfg.set_buffer_config(buffer_config);
        cfg
    }

    /// Create a new config with `addr` and protocol [TCP](Transport::Tcp)
    /// Note: Only the NetworkThread and NetworkDispatcher will use the `BufferConfig`, not Actors
    pub fn with_custom_allocator(
        addr: SocketAddr,
        buffer_config: BufferConfig,
        custom_allocator: Arc<dyn ChunkAllocator>,
    ) -> Self {
        buffer_config.validate();
        NetworkConfig {
            addr,
            transport: Transport::Tcp,
            buffer_config,
            custom_allocator: Some(custom_allocator),
            tcp_nodelay: true,
            max_connection_retry_attempts: defaults::MAX_RETRY_ATTEMPTS,
            connection_retry_interval: defaults::RETRY_CONNECTIONS_INTERVAL,
            boot_timeout: defaults::BOOT_TIMEOUT,
            soft_connection_limit: defaults::SOFT_CONNECTION_LIMIT,
            hard_connection_limit: defaults::HARD_CONNECTION_LIMIT,
        }
    }

    /// Replace the current socket address with `addr`.
    pub fn with_socket(mut self, addr: SocketAddr) -> Self {
        self.addr = addr;
        self
    }

    /// Complete the configuration and provide a function that produces a network dispatcher
    ///
    /// Returns the appropriate function type for use
    /// with [system_components](KompactConfig::system_components).
    pub fn build(self) -> impl Fn(KPromise<()>) -> NetworkDispatcher {
        move |notify_ready| NetworkDispatcher::with_config(self.clone(), notify_ready)
    }

    /// Returns a pointer to the configurations [BufferConfig](net::buffers::BufferConfig).
    pub fn get_buffer_config(&self) -> &BufferConfig {
        &self.buffer_config
    }

    /// Sets the configurations [BufferConfig](net::buffers::BufferConfig) to `buffer_config`
    pub fn set_buffer_config(&mut self, buffer_config: BufferConfig) -> () {
        self.buffer_config = buffer_config;
    }

    /// Returns a pointer to the `CustomAllocator` option so that it can be cloned by the caller.
    pub fn get_custom_allocator(&self) -> &Option<Arc<dyn ChunkAllocator>> {
        &self.custom_allocator
    }

    /// Reads the `tcp_nodelay` parameter of the [NetworkConfig](NetworkConfig).
    pub fn get_tcp_nodelay(&self) -> bool {
        self.tcp_nodelay
    }

    /// If set to `true` the Nagle algorithm will be turned off for all Tcp Network-channels.
    ///
    /// Decreases network-latency at the cost of reduced throughput and increased congestion.
    ///
    /// Default value is `false`, i.e. the Nagle algorithm is turned on by default.
    pub fn set_tcp_nodelay(&mut self, nodelay: bool) {
        self.tcp_nodelay = nodelay;
    }

    /// Configures how many attempts at re-establishing a connection will be made before giving up
    /// and discarding the enqueued outgoing messages.
    ///
    /// Default value is 10 times.
    pub fn set_max_connection_retry_attempts(&mut self, count: u8) {
        self.max_connection_retry_attempts = count;
    }

    /// Returns the number of times the system will retry before giving up on a connection.
    pub fn get_max_connection_retry_attempts(&self) -> u8 {
        self.max_connection_retry_attempts
    }

    /// Configures how long to wait (in ms) between attempts at establishing a connection.
    ///
    /// Default value is 5000 ms.
    pub fn set_connection_retry_interval(&mut self, milliseconds: u64) {
        self.connection_retry_interval = milliseconds;
    }

    /// How long (in ms) the system will wait between attempts at re-establishing connection.
    pub fn get_connection_retry_interval(&self) -> u64 {
        self.connection_retry_interval
    }

    /// Configures how long the system will wait (in ms) for the network layer to set-up
    ///
    /// Default value is 5000 ms.
    pub fn set_boot_timeout(&mut self, milliseconds: u64) {
        self.boot_timeout = milliseconds;
    }

    /// How long (in ms) the system will wait (in ms) for the network layer to set-up
    pub fn get_boot_timeout(&self) -> u64 {
        self.boot_timeout
    }

    /// Configures how many concurrent Network-connections may be active at any point in time
    ///
    /// When the limit is exceeded the system will *gracefully close* the least recently used channel.
    ///
    /// Default value is 1000 Connections.
    pub fn set_soft_connection_limit(&mut self, limit: u32) {
        self.soft_connection_limit = limit;
    }

    /// How many Active Network-connections the system will allow before it starts closing least recently used
    pub fn get_soft_connection_limit(&self) -> u32 {
        self.soft_connection_limit
    }

    /// Configures how many concurrent Network-connections the system may have at any point.
    ///
    /// When the limit is exceeded the system will reject all incoming and outgoing requests for new
    /// connections.
    ///
    /// Default value is 1100 Connections.
    pub fn set_hard_connection_limit(&mut self, limit: u32) {
        self.hard_connection_limit = limit;
    }

    /// How many Network-connections the system will allow before it starts closing least recently used
    pub fn get_hard_connection_limit(&self) -> u32 {
        self.hard_connection_limit
    }
}

/// Socket defaults to `127.0.0.1:0` (i.e. a random local port) and protocol is [TCP](Transport::Tcp)
impl Default for NetworkConfig {
    fn default() -> Self {
        NetworkConfig {
            addr: "127.0.0.1:0".parse().unwrap(),
            transport: Transport::Tcp,
            buffer_config: BufferConfig::default(),
            custom_allocator: None,
            tcp_nodelay: true,
            max_connection_retry_attempts: defaults::MAX_RETRY_ATTEMPTS,
            connection_retry_interval: defaults::RETRY_CONNECTIONS_INTERVAL,
            boot_timeout: defaults::BOOT_TIMEOUT,
            soft_connection_limit: defaults::SOFT_CONNECTION_LIMIT,
            hard_connection_limit: defaults::HARD_CONNECTION_LIMIT,
        }
    }
}

/// A port providing `NetworkStatusUpdates´ to listeners.
pub struct NetworkStatusPort;
impl Port for NetworkStatusPort {
    type Indication = NetworkStatus;
    type Request = NetworkStatusRequest;
}

/// Information regarding changes to the systems connections to remote systems
#[derive(Clone, Debug)]
pub enum NetworkStatus {
    /// Indicates that a connection has been established to the remote system
    ConnectionEstablished(SystemPath, SessionId),
    /// Indicates that a connection has been lost to the remote system.
    /// The system will automatically try to recover the connection for a configurable amount of
    /// retries. The end of the automatic retries is signalled by a `ConnectionDropped` message.
    ConnectionLost(SystemPath, SessionId),
    /// Indicates that a connection has been dropped and no more automatic retries to re-establish
    /// the connection will be attempted and all queued messages have been dropped.
    ConnectionDropped(SystemPath),
    /// Indicates that a connection has been gracefully closed.
    ConnectionClosed(SystemPath, SessionId),
    /// Indicates that a system has been blocked
    BlockedSystem(SystemPath),
    /// Indicates that an IpAddr has been blocked
    BlockedIp(IpAddr),
    /// Indicates that a system has been allowed after previously being blocked
    UnblockedSystem(SystemPath),
    /// Indicates that an IpAddr has been allowed after previously being blocked
    UnblockedIp(IpAddr),
    /// The Soft Connection Limit has been exceeded and the NetworkThread will close
    /// the least recently used connection(s).
    SoftConnectionLimitExceeded,
    /// The Hard Connection Limit has been reached and the NetworkThread will reject both incoming
    /// connection requests and local requests to connect to new hosts
    HardConnectionLimitReached,
}

/// Sent by Actors and Components to request information about the Network
#[derive(Clone, Debug)]
pub enum NetworkStatusRequest {
    /// Request that the connection to the given address is gracefully closed.
    DisconnectSystem(SystemPath),
    /// Request that a connection is established to the given System.
    ConnectSystem(SystemPath),
    /// Request that a SystemPath to be blocked from this system. An established connection
    /// will be dropped and future attempts to establish a connection by that given SystemPath
    /// will be denied.
    BlockSystem(SystemPath),
    /// Request an IpAddr to be blocked.
    BlockIp(IpAddr),
    /// Request a System to be allowed after previously being blocked
    UnblockSystem(SystemPath),
    /// Request an IpAddr to be allowed after previously being blocked
    UnblockIp(IpAddr),
}

/// A network-capable dispatcher for sending messages to remote actors
///
/// Construct this using [NetworkConfig](NetworkConfig::build).
///
/// This dispatcher automatically creates channels to requested target
/// systems on demand and maintains them while in use.
///
/// The current implementation only supports [TCP](Transport::Tcp) as
/// a transport protocol.
///
/// If possible, this implementation will "reflect" messages
/// to local actors directly back up, instead of serialising them first.
#[derive(ComponentDefinition)]
pub struct NetworkDispatcher {
    ctx: ComponentContext<NetworkDispatcher>,
    /// Local map of connection statuses
    connections: NetHashMap<SocketAddr, ConnectionState>,
    /// Network configuration for this dispatcher
    cfg: NetworkConfig,
    /// Shared lookup structure for mapping [actor paths](ActorPath) and [actor refs](ActorRef)
    lookup: Arc<ArcSwap<ActorStore>>,
    // Fields initialized at [Start](ControlEvent::Start) – they require ComponentContextual awareness
    /// Bridge into asynchronous networking layer
    net_bridge: Option<net::Bridge>,
    /// A cached version of the bound system path
    system_path: Option<SystemPath>,
    /// Management for queuing Frames during network unavailability (conn. init. and MPSC unreadiness)
    queue_manager: QueueManager,
    /// Reaper which cleans up deregistered actor references in the actor lookup table
    reaper: lookup::gc::ActorRefReaper,
    notify_ready: Option<KPromise<()>>,
    /// Stores the number of retry-attempts for connections. Checked and incremented periodically by the reaper.
    retry_map: FxHashMap<SocketAddr, u8>,
    garbage_buffers: VecDeque<BufferChunk>,
    /// The dispatcher emits NetworkStatusUpdates to the `NetworkStatusPort`.
    network_status_port: ProvidedPort<NetworkStatusPort>,
}

impl NetworkDispatcher {
    /// Create a new dispatcher with the default configuration
    ///
    /// See also [NetworkConfig](NetworkConfig).
    ///
    /// # Example
    ///
    /// This example binds to local host on a free port chosen by the operating system.
    ///
    /// ```
    /// use kompact::prelude::*;
    ///
    /// let mut conf = KompactConfig::default();
    /// conf.system_components(DeadletterBox::new, NetworkDispatcher::new);
    /// let system = conf.build().expect("system");
    /// # system.shutdown().expect("shutdown");
    /// ```
    pub fn new(notify_ready: KPromise<()>) -> Self {
        let config = NetworkConfig::default();
        NetworkDispatcher::with_config(config, notify_ready)
    }

    /// Create a new dispatcher with the given configuration
    ///
    /// For better readability in combination with [system_components](KompactConfig::system_components),
    /// use [NetworkConfig::build](NetworkConfig::build) instead.
    pub fn with_config(cfg: NetworkConfig, notify_ready: KPromise<()>) -> Self {
        let lookup = Arc::new(ArcSwap::from_pointee(ActorStore::new()));
        // Just a temporary assignment...will be replaced from config on start
        let reaper = lookup::gc::ActorRefReaper::default();

        NetworkDispatcher {
            ctx: ComponentContext::uninitialised(),
            connections: Default::default(),
            cfg,
            lookup,
            net_bridge: None,
            system_path: None,
            queue_manager: QueueManager::new(),
            reaper,
            notify_ready: Some(notify_ready),
            garbage_buffers: VecDeque::new(),
            retry_map: Default::default(),
            network_status_port: ProvidedPort::uninitialised(),
        }
    }

    /// Return a reference to the cached system path
    ///
    /// Mutable, since it will update the cached value, if necessary.
    pub fn system_path_ref(&mut self) -> &SystemPath {
        match self.system_path {
            Some(ref path) => path,
            None => {
                let _ = self.system_path(); // just to fill the cache
                if let Some(ref path) = self.system_path {
                    path
                } else {
                    unreachable!(
                        "Cached value should have been filled by calling self.system_path()!"
                    );
                }
            }
        }
    }

    fn start(&mut self) -> () {
        debug!(self.ctx.log(), "Starting self and network bridge");
        self.reaper = lookup::gc::ActorRefReaper::from_config(self.ctx.config());
        let dispatcher = self
            .actor_ref()
            .hold()
            .expect("Self can hardly be deallocated!");
        let bridge_logger = self.ctx.log().new(o!("owner" => "Bridge"));
        let network_thread_logger = self.ctx.log().new(o!("owner" => "NetworkThread"));
        let (mut bridge, _addr) = net::Bridge::new(
            self.lookup.clone(),
            network_thread_logger,
            bridge_logger,
            self.cfg.addr,
            dispatcher.clone(),
            &self.cfg,
        );

        let deadletter: DynActorRef = self.ctx.system().deadletter_ref().dyn_ref();
        self.lookup.rcu(|current| {
            let mut next = ActorStore::clone(current);
            next.insert(PathResolvable::System, deadletter.clone())
                .expect("Deadletter shouldn't error");
            next
        });

        bridge.set_dispatcher(dispatcher);
        self.schedule_retries();
        self.net_bridge = Some(bridge);
    }

    fn stop(&mut self) -> () {
        if let Some(bridge) = self.net_bridge.take() {
            if let Err(e) = bridge.stop() {
                error!(
                    self.ctx().log(),
                    "NetworkBridge did not shut down as expected! Error was:\n     {:?}\n", e
                );
            }
        }
    }

    fn kill(&mut self) -> () {
        if let Some(bridge) = self.net_bridge.take() {
            if let Err(e) = bridge.kill() {
                error!(
                    self.ctx().log(),
                    "NetworkBridge did not shut down as expected! Error was:\n     {:?}\n", e
                );
            }
        }
    }

    fn schedule_reaper(&mut self) {
        if !self.reaper.is_scheduled() {
            // First time running; mark as scheduled and jump straight to scheduling
            self.reaper.schedule();
        } else {
            // Repeated schedule; prune deallocated ActorRefs and update strategy accordingly
            let num_reaped = self.reaper.run(&self.lookup);
            if num_reaped == 0 {
                // No work done; slow down interval
                self.reaper.strategy_mut().incr();
            } else {
                self.reaper.strategy_mut().decr();
            }
        }
        let next_wakeup = self.reaper.strategy().curr();
        debug!(
            self.ctx().log(),
            "Scheduling reaping at {:?}ms", next_wakeup
        );

        let mut retry_queue = VecDeque::new();
        for mut trash in self.garbage_buffers.drain(..) {
            if !trash.free() {
                retry_queue.push_back(trash);
            }
        }
        // info!(self.ctx().log(), "tried to clean {} buffer(s)", retry_queue.len()); // manual verification in testing
        self.garbage_buffers.append(&mut retry_queue);

        self.schedule_once(Duration::from_millis(next_wakeup), move |target, _id| {
            target.schedule_reaper();
            Handled::Ok
        });
    }

    fn schedule_retries(&mut self) {
        // First check the retry_map if we should re-request connections
        let drain = self.retry_map.clone();
        self.retry_map.clear();
        for (addr, retry) in drain {
            if retry < self.cfg.max_connection_retry_attempts {
                // Make sure we will re-request connection later
                self.retry_map.insert(addr, retry + 1);
                if let Some(bridge) = &self.net_bridge {
                    // Do connection attempt
                    debug!(
                        self.ctx().log(),
                        "Dispatcher retrying connection to host {}, attempt {}/{}",
                        addr,
                        retry,
                        self.cfg.max_connection_retry_attempts
                    );
                    bridge.connect(Transport::Tcp, addr).unwrap();
                }
            } else {
                // Too many retries, give up on the connection.
                info!(
                    self.ctx().log(),
                    "Dispatcher giving up on remote host {}, dropping queues", addr
                );
                self.queue_manager.drop_queue(&addr);
                self.connections.remove(&addr);
                self.network_status_port
                    .trigger(NetworkStatus::ConnectionDropped(SystemPath::with_socket(
                        Transport::Tcp,
                        addr,
                    )));
            }
        }
        self.schedule_once(
            Duration::from_millis(self.cfg.connection_retry_interval),
            move |target, _id| {
                target.schedule_retries();
                Handled::Ok
            },
        );
    }

    fn on_event(&mut self, ev: EventEnvelope) {
        match ev {
            EventEnvelope::Network(ev) => match ev {
                NetworkStatus::ConnectionEstablished(system_path, session) => {
                    self.connection_established(system_path, session)
                }
                NetworkStatus::ConnectionLost(system_path, session) => {
                    self.connection_lost(system_path, session)
                }
                NetworkStatus::ConnectionClosed(system_path, session) => {
                    self.connection_closed(system_path, session)
                }
                NetworkStatus::BlockedSystem(system_path) => {
                    self.connections
                        .insert(system_path.socket_address(), ConnectionState::Blocked);
                    self.network_status_port
                        .trigger(NetworkStatus::BlockedSystem(system_path));
                }
                NetworkStatus::BlockedIp(ip_addr) => {
                    self.network_status_port
                        .trigger(NetworkStatus::BlockedIp(ip_addr));
                }
                NetworkStatus::UnblockedSystem(system_path) => {
                    self.connections.remove(&system_path.socket_address());
                    self.network_status_port
                        .trigger(NetworkStatus::UnblockedSystem(system_path));
                }
                NetworkStatus::UnblockedIp(ip_addr) => {
                    self.network_status_port
                        .trigger(NetworkStatus::UnblockedIp(ip_addr));
                }
                NetworkStatus::SoftConnectionLimitExceeded => self
                    .network_status_port
                    .trigger(NetworkStatus::SoftConnectionLimitExceeded),
                NetworkStatus::HardConnectionLimitReached => self
                    .network_status_port
                    .trigger(NetworkStatus::HardConnectionLimitReached),
                _ => {
                    error!(
                        self.log(),
                        "Unexpected NetworkStatus received by NetworkDispatcher"
                    );
                }
            },
            EventEnvelope::RejectedData((addr, data)) => {
                // These are messages which we routed to a network-thread before they lost the connection.
                self.queue_manager.enqueue_priority_data(*data, addr);
                self.retry_map.entry(addr).or_insert(0);
            }
        }
    }

    fn connection_established(&mut self, system_path: SystemPath, id: SessionId) {
        info!(
            self.ctx().log(),
            "registering newly connected conn at {:?}", system_path
        );
        let addr = &system_path.socket_address();
        self.network_status_port
            .trigger(NetworkStatus::ConnectionEstablished(system_path, id));
        let _ = self.retry_map.remove(addr);
        if self.queue_manager.has_data(addr) {
            // Drain as much as possible
            while let Some(frame) = self.queue_manager.pop_data(addr) {
                if let Some(bridge) = &self.net_bridge {
                    //println!("Sending queued frame to newly established connection");
                    if let Err(e) = bridge.route(*addr, frame, net::Protocol::Tcp) {
                        error!(self.ctx.log(), "Bridge error while routing {:?}", e);
                    }
                }
            }
        }
        self.connections
            .insert(*addr, ConnectionState::Connected(id));
    }

    fn connection_closed(&mut self, system_path: SystemPath, id: SessionId) {
        let addr = &system_path.socket_address();
        self.network_status_port
            .trigger(NetworkStatus::ConnectionClosed(system_path, id));
        // Ack the closing
        if let Some(bridge) = &self.net_bridge {
            if let Err(e) = bridge.ack_closed(*addr) {
                error!(
                    self.ctx.log(),
                    "Bridge error while acking closed connection {:?}", e
                );
            }
        }
        self.connections.insert(*addr, ConnectionState::Closed(id));
        if self.queue_manager.has_data(addr) {
            self.retry_map.insert(*addr, 0);
        }
    }

    fn connection_lost(&mut self, system_path: SystemPath, id: SessionId) {
        let addr = &system_path.socket_address();
        if self.retry_map.get(addr).is_none() {
            warn!(self.ctx().log(), "connection lost to {:?}", addr);
            self.retry_map.insert(*addr, 0); // Make sure we try to re-establish the connection
        }
        self.network_status_port
            .trigger(NetworkStatus::ConnectionLost(system_path, id));
        if let Some(bridge) = &self.net_bridge {
            if let Err(e) = bridge.ack_closed(*addr) {
                error!(
                    self.ctx.log(),
                    "Bridge error while acking lost connection {:?}", e
                );
            }
        }
        self.connections.insert(*addr, ConnectionState::Lost(id));
    }

    /// Forwards `msg` up to a local `dst` actor, if it exists.
    fn route_local(&mut self, dst: ActorPath, msg: DispatchData) -> () {
        let lookup = self.lookup.load();
        let lookup_result = lookup.get_by_actor_path(&dst);
        match msg.into_local() {
            Ok(netmsg) => match lookup_result {
                LookupResult::Ref(actor) => {
                    actor.enqueue(netmsg);
                }
                LookupResult::Group(group) => {
                    group.route(netmsg, self.log());
                }
                LookupResult::None => {
                    error!(
                        self.ctx.log(),
                        "No local actor found at {:?}. Forwarding to DeadletterBox",
                        netmsg.receiver,
                    );
                    self.ctx.deadletter_ref().enqueue(MsgEnvelope::Net(netmsg));
                }
                LookupResult::Err(e) => {
                    error!(
                        self.ctx.log(),
                        "An error occurred during local actor lookup at {:?}. Forwarding to DeadletterBox. The error was: {}",
                        netmsg.receiver,
                        e
                    );
                    self.ctx.deadletter_ref().enqueue(MsgEnvelope::Net(netmsg));
                }
            },
            Err(e) => {
                error!(self.log(), "Could not serialise msg: {:?}. Dropping...", e);
            }
        }
    }

    fn route_remote_udp(
        &mut self,
        addr: SocketAddr,
        data: DispatchData,
    ) -> Result<(), NetworkBridgeErr> {
        if let Some(bridge) = &self.net_bridge {
            bridge.route(addr, data, net::Protocol::Udp)?;
        } else {
            warn!(
                self.ctx.log(),
                "Dropping UDP message to {}, as bridge is not connected.", addr
            );
        }
        Ok(())
    }

    fn route_remote_tcp(
        &mut self,
        addr: SocketAddr,
        data: DispatchData,
    ) -> Result<(), NetworkBridgeErr> {
        let state: &mut ConnectionState =
            self.connections.entry(addr).or_insert(ConnectionState::New);
        let next: Option<ConnectionState> = match *state {
            ConnectionState::New => {
                debug!(
                    self.ctx.log(),
                    "No connection found; establishing and queuing frame"
                );
                self.queue_manager.enqueue_data(data, addr);

                if let Some(ref mut bridge) = self.net_bridge {
                    debug!(self.ctx.log(), "Establishing new connection to {:?}", addr);
                    self.retry_map.insert(addr, 0); // Make sure we will re-request connection later
                    bridge.connect(Transport::Tcp, addr).unwrap();
                    Some(ConnectionState::Initializing)
                } else {
                    error!(self.ctx.log(), "No network bridge found; dropping message");
                    None
                }
            }
            ConnectionState::Connected(_) => {
                if self.queue_manager.has_data(&addr) {
                    self.queue_manager.enqueue_data(data, addr);

                    if let Some(bridge) = &self.net_bridge {
                        while let Some(queued_data) = self.queue_manager.pop_data(&addr) {
                            bridge.route(addr, queued_data, net::Protocol::Tcp)?;
                        }
                    }
                    None
                } else {
                    // Send frame
                    if let Some(bridge) = &self.net_bridge {
                        bridge.route(addr, data, net::Protocol::Tcp)?;
                    }
                    None
                }
            }
            ConnectionState::Initializing => {
                self.queue_manager.enqueue_data(data, addr);
                None
            }
            ConnectionState::Closed(_) => {
                self.queue_manager.enqueue_data(data, addr);
                if let Some(bridge) = &self.net_bridge {
                    self.retry_map.entry(addr).or_insert(0);
                    bridge.connect(Tcp, addr)?;
                }
                Some(ConnectionState::Initializing)
            }
            ConnectionState::Lost(_) => {
                // May be recovered...
                self.queue_manager.enqueue_data(data, addr);
                None
            }
            ConnectionState::Blocked => {
                warn!(
                    self.ctx.log(),
                    "Tried sending a message to a blocked connection: {:?}. Dropping message.",
                    addr
                );
                None
            }
        };

        if let Some(next) = next {
            *state = next;
        }
        Ok(())
    }

    fn resolve_path(&mut self, resolvable: &PathResolvable) -> Result<ActorPath, PathParseError> {
        match resolvable {
            PathResolvable::Path(actor_path) => Ok(actor_path.clone()),
            PathResolvable::Alias(alias) => self
                .system_path()
                .into_named_with_string(alias)
                .map(|p| p.into()),
            PathResolvable::Segments(segments) => self
                .system_path()
                .into_named_with_vec(segments.to_vec())
                .map(|p| p.into()),
            PathResolvable::ActorId(id) => Ok(self.system_path().into_unique(*id).into()),
            PathResolvable::System => Ok(self.deadletter_path()),
        }
    }

    /// Forwards `msg` to destination described by `dst`, routing it across the network
    /// if needed.
    fn route(&mut self, dst: ActorPath, msg: DispatchData) -> Result<(), NetworkBridgeErr> {
        if self.system_path_ref() == dst.system() {
            self.route_local(dst, msg);
            Ok(())
        } else {
            let proto = dst.system().protocol();
            match proto {
                Transport::Local => {
                    self.route_local(dst, msg);
                    Ok(())
                }
                Transport::Tcp => {
                    let addr = SocketAddr::new(*dst.address(), dst.port());
                    self.route_remote_tcp(addr, msg)
                }
                Transport::Udp => {
                    let addr = SocketAddr::new(*dst.address(), dst.port());
                    self.route_remote_udp(addr, msg)
                }
            }
        }
    }

    fn deadletter_path(&mut self) -> ActorPath {
        ActorPath::Named(NamedPath::with_system(self.system_path(), Vec::new()))
    }

    fn register_actor(
        &mut self,
        registration: ActorRegistration,
        update: bool,
        promise: RegistrationPromise,
    ) {
        let ActorRegistration { actor, path } = registration;
        let res = self
            .resolve_path(&path)
            .map_err(RegistrationError::InvalidPath)
            .and_then(|ap| {
                let lease = self.lookup.load();
                if lease.contains(&path) && !update {
                    warn!(
                        self.ctx.log(),
                        "Detected duplicate path during registration. The path will not be re-registered"
                    );
                    drop(lease);
                    Err(RegistrationError::DuplicateEntry)
                } else {
                    drop(lease);
                    let mut result: Result<InsertResult, PathParseError> = Ok(InsertResult::None);
                    self.lookup.rcu(|current| {
                        let mut next = ActorStore::clone(current);
                        result = next.insert(path.clone(), actor.clone());
                        next
                    });
                    if let Ok(ref res) = result {
                        if !res.is_empty() {
                            info!(self.ctx.log(), "Replaced entry for path={:?}", path);
                        }
                    }
                    result.map(|_| ap)
                        .map_err(RegistrationError::InvalidPath)
                }
            });
        if res.is_ok() && !self.reaper.is_scheduled() {
            self.schedule_reaper();
        }
        debug!(self.log(), "Completed actor registration with {:?}", res);
        match promise {
            RegistrationPromise::Fulfil(promise) => {
                promise.fulfil(res).unwrap_or_else(|e| {
                    error!(self.ctx.log(), "Could not notify listeners: {:?}", e)
                });
            }
            RegistrationPromise::None => (), // ignore
        }
    }

    fn register_policy(
        &mut self,
        registration: PolicyRegistration,
        update: bool,
        promise: RegistrationPromise,
    ) {
        let PolicyRegistration { policy, path } = registration;
        let lease = self.lookup.load();
        let path_res = PathResolvable::Segments(path);
        let res = self
            .resolve_path(&path_res)
            .map_err(RegistrationError::InvalidPath)
            .and_then(|ap| {
                if lease.contains(&path_res) && !update {
                    warn!(
                        self.ctx.log(),
                        "Detected duplicate path during registration. The path will not be re-registered",
                    );
                    drop(lease);
                    Err(RegistrationError::DuplicateEntry)
                } else {
                    drop(lease);
                    //let PathResolvable::Segments(path) = path_res;
                    // This should work, we just assigned it above, 
                    // but the Rust compiler can't figure that out
                    let_irrefutable!(path, PathResolvable::Segments(path) = path_res);
                    let mut result: Result<InsertResult, PathParseError> = Ok(InsertResult::None);
                    self.lookup.rcu(|current| {
                        let mut next = ActorStore::clone(current);
                        result = next.set_routing_policy(&path, policy.clone());
                        next
                    });
                    if let Ok(ref res) = result {
                        if !res.is_empty() {
                            info!(self.ctx.log(), "Replaced entry for path={:?}", path);
                        }
                    }
                    result.map(|_| ap).map_err(RegistrationError::InvalidPath)
                }
            });
        debug!(self.log(), "Completed policy registration with {:?}", res);
        match promise {
            RegistrationPromise::Fulfil(promise) => {
                promise.fulfil(res).unwrap_or_else(|e| {
                    error!(self.ctx.log(), "Could not notify listeners: {:?}", e)
                });
            }
            RegistrationPromise::None => (), // ignore
        }
    }

    fn close_channel(&mut self, addr: SocketAddr) -> () {
        if let Some(state) = self.connections.get_mut(&addr) {
            match state {
                ConnectionState::Connected(session) => {
                    trace!(
                        self.ctx.log(),
                        "Closing channel to connected system {}, session {:?}",
                        addr,
                        session
                    );
                    if let Some(bridge) = &self.net_bridge {
                        while self.queue_manager.has_data(&addr) {
                            if let Some(data) = self.queue_manager.pop_data(&addr) {
                                if let Err(e) = bridge.route(addr, data, Protocol::Tcp) {
                                    error!(self.ctx.log(), "Bridge error while routing {:?}", e);
                                }
                            }
                        }
                        if let Err(e) = bridge.close_channel(addr) {
                            error!(self.ctx.log(), "Bridge error closing channel {:?}", e);
                        }
                    }
                }
                _ => {
                    warn!(
                        self.ctx.log(),
                        "Trying to close channel to a system which is not connected {}", addr
                    );
                }
            }
        } else {
            warn!(self.ctx.log(), "Closing channel to unknown system {}", addr);
        }
    }
}

impl Actor for NetworkDispatcher {
    type Message = DispatchEnvelope;

    fn receive_local(&mut self, msg: Self::Message) -> Handled {
        match msg {
            DispatchEnvelope::Msg { src: _, dst, msg } => {
                if let Err(e) = self.route(dst, msg) {
                    error!(self.ctx.log(), "Failed to route message: {:?}", e);
                };
            }
            DispatchEnvelope::ForwardedMsg { msg } => {
                // Look up destination (local or remote), then route or err
                if let Err(e) = self.route(msg.receiver.clone(), DispatchData::NetMessage(msg)) {
                    error!(self.ctx.log(), "Failed to route message: {:?}", e);
                };
            }
            DispatchEnvelope::Registration(reg) => {
                trace!(self.log(), "Got registration request: {:?}", reg);
                let RegistrationEnvelope {
                    event,
                    update,
                    promise,
                } = reg;
                match event {
                    RegistrationEvent::Actor(rea) => self.register_actor(rea, update, promise),
                    RegistrationEvent::Policy(rep) => self.register_policy(rep, update, promise),
                }
            }
            DispatchEnvelope::Event(ev) => self.on_event(ev),
            DispatchEnvelope::LockedChunk(trash) => self.garbage_buffers.push_back(trash),
        }
        Handled::Ok
    }

    fn receive_network(&mut self, msg: NetMessage) -> Handled {
        warn!(self.ctx.log(), "Received network message: {:?}", msg,);
        Handled::Ok
    }
}

impl Dispatcher for NetworkDispatcher {
    /// Generates a [SystemPath](SystemPath) from this dispatcher's configuration
    ///
    /// This is only possible after the socket is bound and will panic if attempted earlier!
    fn system_path(&mut self) -> SystemPath {
        match self.system_path {
            Some(ref path) => path.clone(),
            None => {
                let bound_addr = match self.net_bridge {
                    Some(ref net_bridge) => net_bridge.local_addr().expect("If net bridge is ready, port should be as well!"),
                    None => panic!("You must wait until the socket is bound before attempting to create a system path!"),
                };
                let sp = SystemPath::new(self.cfg.transport, bound_addr.ip(), bound_addr.port());
                self.system_path = Some(sp.clone());
                sp
            }
        }
    }

    fn network_status_port(&mut self) -> &mut ProvidedPort<NetworkStatusPort> {
        &mut self.network_status_port
    }
}

impl ComponentLifecycle for NetworkDispatcher {
    fn on_start(&mut self) -> Handled {
        info!(self.ctx.log(), "Starting network...");
        self.start();
        info!(self.ctx.log(), "Started network just fine.");
        if let Some(promise) = self.notify_ready.take() {
            promise
                .complete()
                .unwrap_or_else(|e| error!(self.ctx.log(), "Could not start network! {:?}", e))
        }
        Handled::Ok
    }

    fn on_stop(&mut self) -> Handled {
        info!(self.ctx.log(), "Stopping network...");
        self.stop();
        info!(self.ctx.log(), "Stopped network.");
        Handled::Ok
    }

    fn on_kill(&mut self) -> Handled {
        info!(self.ctx.log(), "Killing network...");
        self.kill();
        info!(self.ctx.log(), "Killed network.");
        Handled::Ok
    }
}

impl Provide<NetworkStatusPort> for NetworkDispatcher {
    fn handle(&mut self, event: <NetworkStatusPort as Port>::Request) -> Handled {
        debug!(
            self.ctx.log(),
            "Received NetworkStatusPort Request {:?}", event
        );
        match event {
            NetworkStatusRequest::DisconnectSystem(system_path) => {
                self.close_channel(system_path.socket_address());
            }
            NetworkStatusRequest::ConnectSystem(system_path) => {
                if let Some(bridge) = &self.net_bridge {
                    bridge
                        .connect(system_path.protocol(), system_path.socket_address())
                        .unwrap();
                }
            }
            NetworkStatusRequest::BlockIp(ip_addr) => {
                debug!(self.ctx.log(), "Got BlockIp: {:?}", ip_addr);
                if let Some(bridge) = &self.net_bridge {
                    bridge.block_ip(ip_addr).unwrap();
                }
            }
            NetworkStatusRequest::BlockSystem(system_path) => {
                debug!(self.ctx.log(), "Got BlockSystem: {:?}", system_path);
                if let Some(bridge) = &self.net_bridge {
                    bridge.block_socket(system_path.socket_address()).unwrap();
                }
            }
            NetworkStatusRequest::UnblockIp(ip_addr) => {
                debug!(self.ctx.log(), "Got UnblockIp: {:?}", ip_addr);
                if let Some(bridge) = &self.net_bridge {
                    bridge.unblock_ip(ip_addr).unwrap();
                }
            }
            NetworkStatusRequest::UnblockSystem(system_path) => {
                debug!(self.ctx.log(), "Got UnblockSystem: {:?}", system_path);
                if let Some(bridge) = &self.net_bridge {
                    bridge.unblock_socket(system_path.socket_address()).unwrap();
                }
            }
        }
        Handled::Ok
    }
}

/// Helper for forwarding [MsgEnvelope]s to actor references
impl futures::sink::Sink<DispatchEnvelope> for ActorRefStrong<DispatchEnvelope> {
    type Error = ();

    fn poll_ready(self: Pin<&mut Self>, _cx: &mut Context) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn start_send(self: Pin<&mut Self>, item: DispatchEnvelope) -> Result<(), Self::Error> {
        self.tell(item);
        Ok(())
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _cx: &mut Context) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }
}

/// Helper for forwarding [MsgEnvelope]s to actor references
impl futures::sink::Sink<NetMessage> for DynActorRef {
    type Error = ();

    fn poll_ready(self: Pin<&mut Self>, _cx: &mut Context) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn start_send(self: Pin<&mut Self>, item: NetMessage) -> Result<(), Self::Error> {
        DynActorRef::enqueue(&self.as_ref(), item);
        Ok(())
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _cx: &mut Context) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::{super::*, *};
    use crate::prelude_test::net_test_helpers::{PingerAct, PongerAct};
    use std::{thread, time::Duration};

    /*
    // replace ignore with panic cfg gate when https://github.com/rust-lang/rust/pull/74754 is merged
    #[test]
    #[ignore]
    #[should_panic(expected = "KompactSystem: Poisoned")]
    fn failed_network() {
        let mut cfg = KompactConfig::new();
        println!("Configuring network");
        cfg.system_components(DeadletterBox::new, {
            // shouldn't be able to bind on port 80 without root rights
            let net_config =
                NetworkConfig::new("127.0.0.1:80".parse().expect("Address should work"));
            net_config.build()
        });
        println!("Starting KompactSystem");
        let system = cfg.build().expect("KompactSystem");
        thread::sleep(Duration::from_secs(1));
        //unreachable!("System should not start correctly! {:?}", system.label());
        println!("KompactSystem started just fine.");
        let named_path = ActorPath::Named(NamedPath::with_system(
            system.system_path(),
            vec!["test".into()],
        ));
        println!("Got path: {}", named_path);
    }
    */

    #[test]
    fn network_cleanup() {
        let mut cfg = KompactConfig::default();
        println!("Configuring network");
        cfg.system_components(DeadletterBox::new, {
            let net_config =
                NetworkConfig::new("127.0.0.1:0".parse().expect("Address should work"));
            net_config.build()
        });
        println!("Starting KompactSystem");
        let system = cfg.build().expect("KompactSystem");
        println!("KompactSystem started just fine.");
        let named_path = ActorPath::Named(NamedPath::with_system(
            system.system_path(),
            vec!["test".into()],
        ));
        println!("Got path: {}", named_path);
        let port = system.system_path().port();
        println!("Got port: {}", port);
        println!("Shutting down first system...");
        system
            .shutdown()
            .expect("KompactSystem failed to shut down!");
        println!("System shut down.");
        let mut cfg2 = KompactConfig::default();
        println!("Configuring network");
        cfg2.system_components(DeadletterBox::new, {
            let net_config =
                NetworkConfig::new(SocketAddr::new("127.0.0.1".parse().unwrap(), port));
            net_config.build()
        });
        println!("Starting 2nd KompactSystem");
        let system2 = cfg2.build().expect("KompactSystem");
        thread::sleep(Duration::from_millis(100));
        println!("2nd KompactSystem started just fine.");
        let named_path2 = ActorPath::Named(NamedPath::with_system(
            system2.system_path(),
            vec!["test".into()],
        ));
        println!("Got path: {}", named_path);
        assert_eq!(named_path, named_path2);
        system2
            .shutdown()
            .expect("2nd KompactSystem failed to shut down!");
    }

    /// This is similar to network_cleanup test that will trigger a failed binding.
    /// The retry should occur when system2 is building and should succeed after system1 is killed.
    #[test]
    fn network_cleanup_with_timeout() {
        let mut cfg = KompactConfig::default();
        println!("Configuring network");
        cfg.system_components(DeadletterBox::new, {
            let net_config =
                NetworkConfig::new("127.0.0.1:0".parse().expect("Address should work"));
            net_config.build()
        });
        println!("Starting KompactSystem");
        let system = cfg.build().expect("KompactSystem");
        println!("KompactSystem started just fine.");
        let named_path = ActorPath::Named(NamedPath::with_system(
            system.system_path(),
            vec!["test".into()],
        ));
        println!("Got path: {}", named_path);
        let port = system.system_path().port();
        println!("Got port: {}", port);

        thread::Builder::new()
            .name("System1 Killer".to_string())
            .spawn(move || {
                thread::sleep(Duration::from_millis(100));
                println!("Shutting down first system...");
                system
                    .shutdown()
                    .expect("KompactSystem failed to shut down!");
                println!("System shut down.");
            })
            .ok();

        let mut cfg2 = KompactConfig::default();
        println!("Configuring network");
        cfg2.system_components(DeadletterBox::new, {
            let net_config =
                NetworkConfig::new(SocketAddr::new("127.0.0.1".parse().unwrap(), port));
            net_config.build()
        });
        println!("Starting 2nd KompactSystem");
        let system2 = cfg2.build().expect("KompactSystem");
        thread::sleep(Duration::from_millis(100));
        println!("2nd KompactSystem started just fine.");
        let named_path2 = ActorPath::Named(NamedPath::with_system(
            system2.system_path(),
            vec!["test".into()],
        ));
        println!("Got path: {}", named_path);
        assert_eq!(named_path, named_path2);
        system2
            .shutdown()
            .expect("2nd KompactSystem failed to shut down!");
    }

    #[test]
    fn test_system_path_timing() {
        let mut cfg = KompactConfig::default();
        println!("Configuring network");
        cfg.system_components(DeadletterBox::new, NetworkConfig::default().build());
        println!("Starting KompactSystem");
        let system = cfg.build().expect("KompactSystem");
        println!("KompactSystem started just fine.");
        let named_path = ActorPath::Named(NamedPath::with_system(
            system.system_path(),
            vec!["test".into()],
        ));
        println!("Got path: {}", named_path);
        // if nothing panics the test succeeds
    }

    #[test]
    // Identical with `remote_lost_and_continued_connection` up to the final sleep time and assertion
    // system1 times out in its reconnection attempts and drops the enqueued buffers.
    // After indirectly asserting that the queue was dropped we start up a new pinger, and assert that it succeeds.
    fn cleanup_bufferchunks_from_dead_actors() {
        let system1 = || {
            let mut cfg = KompactConfig::default();
            cfg.system_components(
                DeadletterBox::new,
                NetworkConfig::new("127.0.0.1:0".parse().expect("Address should work")).build(),
            );
            cfg.build().expect("KompactSystem")
        };
        let system2 = |port| {
            let mut cfg = KompactConfig::default();
            cfg.system_components(
                DeadletterBox::new,
                NetworkConfig::new(SocketAddr::new("127.0.0.1".parse().unwrap(), port)).build(),
            );
            cfg.build().expect("KompactSystem")
        };

        // Set-up system2a
        let system2a = system2(0);
        let port = system2a.system_path().port();
        //let (ponger_unique, pouf) = remote.create_and_register(PongerAct::new);
        let (ponger_named, ponf) = system2a.create_and_register(PongerAct::new_lazy);
        let poaf = system2a.register_by_alias(&ponger_named, "custom_name");
        ponf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
        poaf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
        let named_path = ActorPath::Named(NamedPath::with_system(
            system2a.system_path(),
            vec!["custom_name".into()],
        ));
        let named_path_clone = named_path;
        // Set-up system1
        let system1: KompactSystem = system1();
        let (pinger_named, pinf) =
            system1.create_and_register(move || PingerAct::new_eager(named_path_clone));
        pinf.wait_expect(Duration::from_millis(1000), "Pinger failed to register!");

        // Kill system2a
        system2a.shutdown().ok();
        // Start system1
        system1.start(&pinger_named);
        // Wait for the pings to be sent from the actor to the NetworkDispatch and onto the Thread
        thread::sleep(Duration::from_millis(100));
        // Kill the actor and wait for its BufferChunk to reach the NetworkDispatch and let the reaping try at least once
        system1.kill(pinger_named);

        // TODO no sleeps!
        thread::sleep(Duration::from_millis(5000));

        // Assertion 1: The Network_Dispatcher on system1 has >0 buffers to cleanup
        let mut garbage_len = 0;
        let sc: &dyn SystemComponents = system1.get_system_components();
        if let Some(cc) = sc.downcast::<CustomComponents<DeadletterBox, NetworkDispatcher>>() {
            garbage_len = cc.dispatcher.on_definition(|nd| nd.garbage_buffers.len());
        }
        assert_ne!(0, garbage_len);

        // Start up system2b
        println!("Setting up system2b");
        let system2b = system2(port);
        let (ponger_named, ponf) = system2b.create_and_register(PongerAct::new_lazy);
        let poaf = system2b.register_by_alias(&ponger_named, "custom_name");
        ponf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
        poaf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
        println!("Starting actor on system2b");
        system2b.start(&ponger_named);

        // We give the connection plenty of time to re-establish and transfer it's old queue and cleanup the BufferChunk
        // TODO no sleeps!
        thread::sleep(Duration::from_millis(10000));

        // Assertion 2: The Network_Dispatcher on system1 now has 0 buffers to cleanup.
        if let Some(cc) = sc.downcast::<CustomComponents<DeadletterBox, NetworkDispatcher>>() {
            garbage_len = cc.dispatcher.on_definition(|nd| nd.garbage_buffers.len());
        }
        assert_eq!(0, garbage_len);

        system1
            .shutdown()
            .expect("Kompact didn't shut down properly");
        system2b
            .shutdown()
            .expect("Kompact didn't shut down properly");
    }
}

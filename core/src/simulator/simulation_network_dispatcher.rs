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
use ipnet::IpNet;
use dispatch::lookup::{ActorLookup, ActorStore, InsertResult, LookupResult};
use dispatch::queue_manager::QueueManager;
use rustc_hash::FxHashMap;
use std::{collections::VecDeque, net::IpAddr, time::Duration};
use simulation_bridge::SimulationBridge;

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
/// conf.system_components(DeadletterBox::new, SimulationNetworkConfig::default().build());
/// let system = conf.build().expect("system");
/// # system.shutdown().expect("shutdown");
/// ```
#[derive(Clone, Debug)]
pub struct SimulationNetworkConfig {
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

impl SimulationNetworkConfig {
    /// Create a new config with `addr` and protocol [TCP](Transport::Tcp)
    /// SimulationNetworkDispatcher and NetworkThread will use the default `BufferConfig`
    pub fn new(addr: SocketAddr) -> Self {
        SimulationNetworkConfig {
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
    /// Note: Only the NetworkThread and SimulationNetworkDispatcher will use the `BufferConfig`, not Actors
    pub fn with_buffer_config(addr: SocketAddr, buffer_config: BufferConfig) -> Self {
        buffer_config.validate();
        let mut cfg = SimulationNetworkConfig::new(addr);
        cfg.set_buffer_config(buffer_config);
        cfg
    }

    /// Create a new config with `addr` and protocol [TCP](Transport::Tcp)
    /// Note: Only the NetworkThread and SimulationNetworkDispatcher will use the `BufferConfig`, not Actors
    pub fn with_custom_allocator(
        addr: SocketAddr,
        buffer_config: BufferConfig,
        custom_allocator: Arc<dyn ChunkAllocator>,
    ) -> Self {
        buffer_config.validate();
        SimulationNetworkConfig {
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
    pub fn build(self, network: Arc<Mutex<SimulationNetwork>>) -> impl Fn(KPromise<()>) -> SimulationNetworkDispatcher {
        move |notify_ready| SimulationNetworkDispatcher::with_config(self.clone(), notify_ready, network.clone())
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

    /// Reads the `tcp_nodelay` parameter of the [SimulationNetworkConfig](SimulationNetworkConfig).
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
impl Default for SimulationNetworkConfig {
    fn default() -> Self {
        SimulationNetworkConfig {
            addr: "127.0.0.1:0".parse().unwrap(),
            transport: Transport::Udp,
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

#[derive(ComponentDefinition)]
pub struct SimulationNetworkDispatcher {
    ctx: ComponentContext<SimulationNetworkDispatcher>,
    /// Local map of connection statuses
    /// Maybe just store a queue instead of connectionstate
    connections: NetHashMap<SocketAddr, ConnectionState>,
    /// Network configuration for this dispatcher
    cfg: SimulationNetworkConfig,
    /// Shared lookup structure for mapping [actor paths](ActorPath) and [actor refs](ActorRef)
    /// send messages for local actors 
    lookup: Arc<ArcSwap<ActorStore>>,
    // Fields initialized at [Start](ControlEvent::Start) – they require ComponentContextual awareness
    /// Bridge into asynchronous networking layer
    net_bridge: Option<SimulationBridge>,
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
    network: Arc<Mutex<SimulationNetwork>>
}

impl Actor for SimulationNetworkDispatcher{
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
            DispatchEnvelope::Event(ev) => todo!(),
            DispatchEnvelope::LockedChunk(trash) => self.garbage_buffers.push_back(trash),
        }
        Handled::Ok
    }

    fn receive_network(&mut self, msg: NetMessage) -> Handled {
        //warn!("{} {} {}", self.ctx.log(), "Received network message: {}", msg,);
        Handled::Ok
    }
}

impl Dispatcher for SimulationNetworkDispatcher {
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
                println!("SYSPATH {} {} {}", self.cfg.transport, bound_addr.ip(), bound_addr.port());
                let sp = SystemPath::new(self.cfg.transport, bound_addr.ip(), bound_addr.port());
                println!("SYSTEMPATH IS BEING SET SIM");
                self.system_path = Some(sp.clone());
                sp
            }
        }
    }

    fn network_status_port(&mut self) -> &mut ProvidedPort<NetworkStatusPort> {
        //&mut self.network_status_port
        todo!();
    }
}

impl ComponentLifecycle for SimulationNetworkDispatcher {
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

impl SimulationNetworkDispatcher {
    /// Create a new dispatcher with the given configuration
    ///
    /// For better readability in combination with [system_components](KompactConfig::system_components),
    /// use [NetworkConfig::build](NetworkConfig::build) instead.
    pub fn with_config(cfg: SimulationNetworkConfig, notify_ready: KPromise<()>, network: Arc<Mutex<SimulationNetwork>>) -> Self {
        let lookup = Arc::new(ArcSwap::from_pointee(ActorStore::new()));
        // Just a temporary assignment...will be replaced from config on start
        let reaper = lookup::gc::ActorRefReaper::default();

        SimulationNetworkDispatcher {
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
            network
        }
    }

    fn start(&mut self) -> () {
        //debug!(self.ctx.log(), "Starting self and network bridge");
        self.reaper = lookup::gc::ActorRefReaper::from_config(self.ctx.config());
        self.start_bridge(self.cfg.addr);
    
        let deadletter: DynActorRef = self.ctx.system().deadletter_ref().dyn_ref();
        self.lookup.rcu(|current| {
            let mut next = ActorStore::clone(current);
            next.insert(PathResolvable::System, deadletter.clone())
                .expect("Deadletter shouldn't error");
            next
        });
    
        self.schedule_retries();
    }

    fn stop(&mut self) -> (){
        todo!();
    }

    fn kill(&mut self) -> (){
        todo!();
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
                    /*debug!(
                        self.ctx().log(),
                        "Dispatcher retrying connection to host {}, attempt {}/{}",
                        addr,
                        retry,
                        self.cfg.max_connection_retry_attempts
                    );*/
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
                /*self.network_status_port
                    .trigger(NetworkStatus::ConnectionDropped(SystemPath::with_socket(
                        Transport::Tcp,
                        addr,
                    )));*/
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

    //Needs to adapt to the new bridge implementation.
    fn start_bridge(&mut self, address: SocketAddr) -> () {
        let dispatcher = self
            .actor_ref()
            .hold()
            .expect("Self can hardly be deallocated!");
        let bridge_logger = self.ctx.log().new(o!("owner" => "Bridge"));
        let network_thread_logger = self.ctx.log().new(o!("owner" => "NetworkThread"));
        let (mut bridge, _addr) = SimulationBridge::new(
            self.lookup.clone(),
            network_thread_logger,
            bridge_logger,
            address,
            dispatcher.clone(),
            &self.cfg,
            self.network.clone()
        );
        bridge.set_dispatcher(dispatcher);
        self.net_bridge = Some(bridge);
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

    fn route(&mut self, dst: ActorPath, msg: DispatchData) -> Result<(), NetworkBridgeErr> {
        if self.system_path_ref() == dst.system() {
            //println!("Routing to same system!");
            self.route_local(dst, msg);
            Ok(())
        } else {
            let proto = dst.system().protocol();
            match proto {
                Transport::Local => {
                    // local!");
                    self.route_local(dst, msg);
                    Ok(())
                }
                Transport::Tcp => {
                    //println!("Routing tcp!");
                    let addr = SocketAddr::new(*dst.address(), dst.port());
                    self.route_remote_tcp(addr, msg)
                }
                Transport::Udp => {
                    //println!("Routing udp!");
                    let addr = SocketAddr::new(*dst.address(), dst.port());
                    self.route_remote_udp(addr, msg)
                }
            }
        }
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
        //println!("ROUTE REMOTE UDP");
        if let Some(bridge) = &self.net_bridge {
            bridge.route(addr, data, net::Protocol::Udp)?;
        } else {
            /* 
                        warn!(
                "{} {} {}", self.ctx.log(),
                "Dropping UDP message to {}, as bridge is not .", addr
            );connected
            */
        }
        Ok(())
    }

    fn route_remote_tcp(
        &mut self,
        addr: SocketAddr,
        data: DispatchData,
    ) -> Result<(), NetworkBridgeErr> {
        //println!("ROUTE REMOTE TCP");

        let state: &mut ConnectionState =
            self.connections.entry(addr).or_insert(ConnectionState::New);
        let next: Option<ConnectionState> = match *state {
            ConnectionState::New => {
                /*
                debug!(
                    "{} {}", self.ctx.log(),
                    "No connection found; establishing and queuing frame"
                ); */
                self.queue_manager.enqueue_data(data, addr);

                if let Some(ref mut bridge) = self.net_bridge {
                    //debug!("{} {} {}", self.ctx.log(), "Establishing new connection to {:?}", addr);
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
                /* 
                                warn!(
                    self.ctx.log(),
                    "Tried sending a message to a blocked connection: {:?}. Dropping message.",
                    addr
                );
                */
                None
            }
        };

        if let Some(next) = next {
            *state = next;
        }
        Ok(())
    }

    //REGISTER IN SIMULATION BRIDGE
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
                    /*warn!(
                        self.ctx.log(),
                        "Detected duplicate path during registration. The path will not be re-registered"
                    );*/
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
        //debug!(self.log(), "Completed actor registration with {:?}", res);
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
                    /* warn!(
                        self.ctx.log(),
                        "Detected duplicate path during registration. The path will not be re-registered",
                    );*/
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
        //debug!(self.log(), "Completed policy registration with {:?}", res);
        match promise {
            RegistrationPromise::Fulfil(promise) => {
                promise.fulfil(res).unwrap_or_else(|e| {
                    error!(self.ctx.log(), "Could not notify listeners: {:?}", e)
                });
            }
            RegistrationPromise::None => (), // ignore
        }
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

    fn deadletter_path(&mut self) -> ActorPath {
        ActorPath::Named(NamedPath::with_system(self.system_path(), Vec::new()))
    }

    fn schedule_reaper(&mut self) {
        if !self.reaper.is_scheduled() {
            // First time running; mark as scheduled and jump straight to scheduling
            println!("schedule_reaper");
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
        /*debug!(
            self.ctx().log(),
            "Scheduling reaping at {:?}ms", next_wakeup
        );*/

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

    /*fn get_socket_addr(&self) -> &Option<SocketAddr>{
        match self.net_bridge{
            Some(bridge) => bridge.local_addr(),
            None => todo!(),
        }        
    }

    fn get_actor_store(&self) -> {
        match self.net_bridge{
            Some(bridge) => bridge.local_addr(),
            None => todo!(),
        }      
    }*/
}
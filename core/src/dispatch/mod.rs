use super::*;

use crate::{
    actors::{Actor, ActorPath, Dispatcher, DynActorRef, SystemPath, Transport},
    component::{Component, ComponentContext, ExecuteResult, Provide},
    lifecycle::{ControlEvent, ControlPort},
};
use std::{net::SocketAddr, pin::Pin, sync::Arc};

use crate::{
    actors::{NamedPath, UniquePath},
    dispatch::{lookup::ActorStore, queue_manager::QueueManager},
    messaging::{
        DispatchData,
        DispatchEnvelope,
        EventEnvelope,
        MsgEnvelope,
        NetMessage,
        PathResolvable,
        RegistrationEnvelope,
        RegistrationError,
        RegistrationPromise,
        RegistrationResponse,
        SerialisedFrame,
    },
    net::{
        buffer::{BufferEncoder, EncodeBuffer},
        events::NetworkEvent,
        ConnectionState,
        NetworkBridgeErr,
    },
    timer::timer_manager::Timer,
};
use arc_swap::ArcSwap;
use fnv::FnvHashMap;
use futures::{
    self,
    task::{Context, Poll},
};
use std::{io::ErrorKind, time::Duration};

pub mod lookup;
pub mod queue_manager;

const RETRY_CONNECTIONS_INTERVAL: u64 = 5000;
const MAX_RETRY_ATTEMPTS: u8 = 10;

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
#[derive(Clone, PartialEq, Debug)]
pub struct NetworkConfig {
    addr: SocketAddr,
    transport: Transport,
}

impl NetworkConfig {
    /// Create a new config with `addr` and protocol [TCP](Transport::TCP)
    pub fn new(addr: SocketAddr) -> Self {
        NetworkConfig {
            addr,
            transport: Transport::TCP,
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
    pub fn build(self) -> impl Fn(Promise<()>) -> NetworkDispatcher {
        move |notify_ready| NetworkDispatcher::with_config(self.clone(), notify_ready)
    }
}

/// Socket defaults to `127.0.0.1:0` (i.e. a random local port) and protocol is [TCP](Transport::TCP)
impl Default for NetworkConfig {
    fn default() -> Self {
        NetworkConfig {
            addr: "127.0.0.1:0".parse().unwrap(),
            transport: Transport::TCP,
        }
    }
}

/// A network-capable dispatcher for sending messages to remote actors
///
/// Construct this using [NetworkConfig](NetworkConfig::build).
///
/// This dispatcher automatically creates channels to requested target
/// systems on demand and maintains them while in use.
///
/// The current implementation only supports [TCP](Transport::TCP) as
/// a transport protocol.
///
/// If possible, this implementation will "reflect" messages
/// to local actors directly back up, instead of serialising them first.
#[derive(ComponentDefinition)]
pub struct NetworkDispatcher {
    ctx: ComponentContext<NetworkDispatcher>,
    /// Local map of connection statuses
    connections: FnvHashMap<SocketAddr, ConnectionState>,
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
    notify_ready: Option<Promise<()>>,
    encode_buffer: EncodeBuffer,
    /// Stores the number of retry-attempts for connections. Checked and incremented periodically by the reaper.
    retry_map: FnvHashMap<SocketAddr, (u8)>,
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
    pub fn new(notify_ready: Promise<()>) -> Self {
        let config = NetworkConfig::default();
        NetworkDispatcher::with_config(config, notify_ready)
    }

    /// Create a new dispatcher with the given configuration
    ///
    /// For better readability in combination with [system_components](KompactConfig::system_components),
    /// use [NetworkConfig::build](NetworkConfig::build) instead.
    pub fn with_config(cfg: NetworkConfig, notify_ready: Promise<()>) -> Self {
        let lookup = Arc::new(ArcSwap::from(Arc::new(ActorStore::new())));
        let reaper = lookup::gc::ActorRefReaper::default();
        let encode_buffer = crate::net::buffer::EncodeBuffer::new();

        NetworkDispatcher {
            ctx: ComponentContext::uninitialised(),
            connections: FnvHashMap::default(),
            cfg,
            lookup,
            net_bridge: None,
            system_path: None,
            queue_manager: QueueManager::new(),
            reaper,
            notify_ready: Some(notify_ready),
            encode_buffer,
            retry_map: FnvHashMap::default(),
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

    fn start(&mut self) -> Result<(), net::NetworkBridgeErr> {
        debug!(self.ctx.log(), "Starting self and network bridge");
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
        );

        bridge.set_dispatcher(dispatcher);
        self.schedule_retries();
        self.net_bridge = Some(bridge);
        Ok(())
    }

    fn stop(&mut self) -> () {
        self.do_stop(false)
    }

    fn kill(&mut self) -> () {
        self.do_stop(true)
    }

    fn do_stop(&mut self, _cleanup: bool) -> () {
        if let Some(bridge) = self.net_bridge.take() {
            if let Err(e) = bridge.stop() {
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
            if retry < MAX_RETRY_ATTEMPTS {
                // Make sure we will re-request connection later
                self.retry_map.insert(addr, retry + 1);
                if let Some(bridge) = &self.net_bridge {
                    // Do connection attempt
                    debug!(
                        self.ctx().log(),
                        "Dispatcher retrying connection to host {}, attempt {}/{}",
                        addr,
                        retry,
                        MAX_RETRY_ATTEMPTS
                    );
                    bridge.connect(Transport::TCP, addr).unwrap();
                }
            } else {
                // Too many retries, give up on the connection.
                info!(
                    self.ctx().log(),
                    "Dispatcher giving up on remote host {}, dropping queues", addr
                );
                self.queue_manager.drop_queue(&addr);
                self.connections.remove(&addr);
            }
        }
        self.schedule_once(
            Duration::from_millis(RETRY_CONNECTIONS_INTERVAL),
            move |target, _id| {
                target.schedule_retries();
                Handled::Ok
            },
        );
    }

    fn on_event(&mut self, ev: EventEnvelope) {
        match ev {
            EventEnvelope::Network(ev) => match ev {
                NetworkEvent::Connection(addr, conn_state) => {
                    if let Err(e) = self.on_conn_state(addr, conn_state) {
                        error!(
                            self.ctx().log(),
                            "Error while connecting to {}, \n{:?}", addr, e
                        )
                    }
                }
                NetworkEvent::Data(_) => {
                    // TODO shouldn't be receiving these here, as they should be routed directly to the ActorRef
                    debug!(self.ctx().log(), "Received important data!");
                }
                NetworkEvent::RejectedFrame(addr, frame) => {
                    // These are messages which we routed to a network-thread before they lost the connection.
                    self.queue_manager.enqueue_priority_frame(frame, addr);
                }
            },
        }
    }

    fn on_conn_state(
        &mut self,
        addr: SocketAddr,
        mut state: ConnectionState,
    ) -> Result<(), NetworkBridgeErr> {
        use self::ConnectionState::*;
        match state {
            Connected(ref mut _frame_sender) => {
                info!(
                    self.ctx().log(),
                    "registering newly connected conn at {:?}", addr
                );
                let _ = self.retry_map.remove(&addr);
                if self.queue_manager.has_frame(&addr) {
                    // Drain as much as possible
                    while let Some(frame) = self.queue_manager.pop_frame(&addr) {
                        if let Some(bridge) = &self.net_bridge {
                            //println!("Sending queued frame to newly established connection");
                            bridge.route(addr, frame)?;
                        }
                    }
                }
            }
            Closed => {
                if self.retry_map.get(&addr).is_none() {
                    warn!(self.ctx().log(), "connection closed for {:?}", addr);
                    self.retry_map.insert(addr, 0); // Make sure we try to re-establish the connection
                }
            }
            Error(ref err) => {
                match err {
                    x if x.kind() == ErrorKind::ConnectionRefused => {
                        error!(self.ctx().log(), "connection refused for {:?}", addr);
                        // TODO determine how we want to proceed
                        // If TCP, the network bridge has already attempted retries with exponential
                        // backoff according to its configuration.
                    }
                    why => {
                        error!(
                            self.ctx().log(),
                            "connection error for {:?}: {:?}", addr, why
                        );
                    }
                }
            }
            ref _other => (), // Don't care
        }
        self.connections.insert(addr, state);
        Ok(())
    }

    /// Forwards `msg` up to a local `dst` actor, if it exists.
    fn route_local<R>(&mut self, msg: R) -> Result<(), NetworkBridgeErr>
    where
        R: Routable,
    {
        use crate::dispatch::lookup::ActorLookup;
        let lookup = self.lookup.lease();
        let actor_opt = lookup.get_by_actor_path(msg.destination());
        match msg.to_local() {
            Ok(netmsg) => {
                if let Some(ref actor) = actor_opt {
                    actor.enqueue(netmsg);
                    Ok(())
                } else {
                    error!(
                        self.ctx.log(),
                        "No local actor found at {:?}. Forwarding to DeadletterBox",
                        netmsg.receiver,
                    );
                    self.ctx.deadletter_ref().enqueue(MsgEnvelope::Net(netmsg));
                    Ok(())
                }
            }
            Err(e) => {
                error!(self.log(), "Could not serialise msg: {:?}. Dropping...", e);
                Ok(()) // consider this an ok:ish result
            }
        }
    }

    /// Routes the provided message to the destination, or queues the message until the connection
    /// is available.
    fn route_remote<R>(&mut self, msg: R) -> Result<(), NetworkBridgeErr>
    where
        R: Routable,
    {
        let dst = msg.destination();
        let addr = SocketAddr::new(*dst.address(), dst.port());
        let buf = &mut BufferEncoder::new(&mut self.encode_buffer);
        let serialised = msg.to_serialised(buf)?;

        let state: &mut ConnectionState =
            self.connections.entry(addr).or_insert(ConnectionState::New);
        let next: Option<ConnectionState> = match *state {
            ConnectionState::New => {
                debug!(
                    self.ctx.log(),
                    "No connection found; establishing and queuing frame"
                );
                self.queue_manager.enqueue_frame(serialised, addr);

                if let Some(ref mut bridge) = self.net_bridge {
                    debug!(self.ctx.log(), "Establishing new connection to {:?}", addr);
                    self.retry_map.insert(addr, 0); // Make sure we will re-request connection later
                    bridge.connect(Transport::TCP, addr).unwrap();
                    Some(ConnectionState::Initializing)
                } else {
                    error!(self.ctx.log(), "No network bridge found; dropping message");
                    Some(ConnectionState::Closed)
                }
            }
            ConnectionState::Connected(_) => {
                if self.queue_manager.has_frame(&addr) {
                    self.queue_manager.enqueue_frame(serialised, addr);

                    if let Some(bridge) = &self.net_bridge {
                        while let Some(frame) = self.queue_manager.pop_frame(&addr) {
                            bridge.route(addr, frame)?;
                        }
                    }
                    None
                } else {
                    // Send frame
                    if let Some(bridge) = &self.net_bridge {
                        bridge.route(addr, serialised)?;
                    }
                    None
                }
            }
            ConnectionState::Initializing => {
                //debug!(self.ctx.log(), "Connection is initializing; queuing frame");
                self.queue_manager.enqueue_frame(serialised, addr);
                None
            }
            ConnectionState::Closed => {
                // Enqueue the Frame. The connection will sort itself out or drop the queue eventually
                self.queue_manager.enqueue_frame(serialised, addr);
                None
            }
            _ => None,
        };

        if let Some(next) = next {
            *state = next;
        }
        Ok(())
    }

    fn resolve_path(&mut self, resolvable: &PathResolvable) -> ActorPath {
        match resolvable {
            PathResolvable::Path(actor_path) => actor_path.clone(),
            PathResolvable::Alias(alias) => ActorPath::Named(NamedPath::with_system(
                self.system_path(),
                vec![alias.clone()],
            )),
            PathResolvable::ActorId(uuid) => {
                ActorPath::Unique(UniquePath::with_system(self.system_path(), *uuid))
            }
            PathResolvable::System => self.actor_path(),
        }
    }

    /// Forwards `msg` to destination described by `dst`, routing it across the network
    /// if needed.
    fn route<R>(&mut self, msg: R) -> Result<(), NetworkBridgeErr>
    where
        R: Routable,
    {
        if self.system_path_ref() == msg.destination().system() {
            self.route_local(msg)
        } else {
            let proto = msg.destination().system().protocol();
            match proto {
                Transport::LOCAL => self.route_local(msg),
                Transport::TCP => self.route_remote(msg),
                Transport::UDP => Err(NetworkBridgeErr::Other(
                    "UDP routing is not supported.".to_string(),
                )),
            }
        }
    }

    fn actor_path(&mut self) -> ActorPath {
        let uuid = *self.ctx.id();
        ActorPath::Unique(UniquePath::with_system(self.system_path(), uuid))
    }
}

impl Actor for NetworkDispatcher {
    type Message = DispatchEnvelope;

    fn receive_local(&mut self, msg: Self::Message) -> Handled {
        match msg {
            DispatchEnvelope::Msg { src, dst, msg } => {
                // Look up destination (local or remote), then route or err
                let resolved_src = self.resolve_path(&src);
                if let Err(e) = self.route((resolved_src, dst, msg)) {
                    error!(self.ctx.log(), "Failed to route message: {:?}", e);
                };
            }
            DispatchEnvelope::ForwardedMsg { msg } => {
                // Look up destination (local or remote), then route or err
                if let Err(e) = self.route(msg) {
                    error!(self.ctx.log(), "Failed to route message: {:?}", e);
                };
            }
            DispatchEnvelope::Registration(reg) => {
                use lookup::ActorLookup;

                match reg {
                    RegistrationEnvelope {
                        actor,
                        path,
                        update,
                        promise,
                    } => {
                        let ap = self.resolve_path(&path);
                        let lease = self.lookup.lease();
                        let res = if lease.contains(&path) && !update {
                            warn!(self.ctx.log(), "Detected duplicate path during registration. The path will not be re-registered");
                            drop(lease);
                            Err(RegistrationError::DuplicateEntry)
                        } else {
                            drop(lease);
                            let mut replaced = false;
                            self.lookup.rcu(|current| {
                                let mut next = (*current).clone();
                                if let Some(_old_entry) = next.insert(actor.clone(), path.clone()) {
                                    replaced = true;
                                }
                                Arc::new(next)
                            });
                            if replaced {
                                info!(self.ctx.log(), "Replaced entry for path={:?}", path.clone());
                            }
                            Ok(ap)
                        };

                        if res.is_ok() && !self.reaper.is_scheduled() {
                            self.schedule_reaper();
                        }
                        match promise {
                            RegistrationPromise::Fulfil(promise) => {
                                promise.fulfil(res).unwrap_or_else(|e| {
                                    error!(self.ctx.log(), "Could not notify listeners: {:?}", e)
                                });
                            }
                            RegistrationPromise::Reply { id, recipient } => {
                                let msg = RegistrationResponse::new(id, res);
                                recipient.tell(msg);
                            }
                            RegistrationPromise::None => (), // ignore
                        }
                    }
                }
            }
            DispatchEnvelope::Event(ev) => self.on_event(ev),
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
                    Some(ref net_bridge) => net_bridge.local_addr().clone().expect("If net bridge is ready, port should be as well!"),
                    None => panic!("You must wait until the socket is bound before attempting to create a system path!"),
                };
                let sp = SystemPath::new(self.cfg.transport, bound_addr.ip(), bound_addr.port());
                self.system_path = Some(sp.clone());
                sp
            }
        }
    }
}

impl Provide<ControlPort> for NetworkDispatcher {
    fn handle(&mut self, event: ControlEvent) -> Handled {
        match event {
            ControlEvent::Start => {
                info!(self.ctx.log(), "Starting network...");
                let res = self.start(); //.expect("Could not create NetworkDispatcher!");
                match res {
                    Ok(_) => {
                        info!(self.ctx.log(), "Started network just fine.");
                        match self.notify_ready.take() {
                            Some(promise) => promise.fulfil(()).unwrap_or_else(|e| {
                                error!(self.ctx.log(), "Could not start network! {:?}", e)
                            }),
                            None => (),
                        }
                    }
                    Err(e) => {
                        error!(self.ctx.log(), "Could not start network! {:?}", e);
                        panic!("Kill me now!");
                    }
                }
            }
            ControlEvent::Stop => {
                info!(self.ctx.log(), "Stopping network...");
                self.stop();
                info!(self.ctx.log(), "Stopped network.");
            }
            ControlEvent::Kill => {
                info!(self.ctx.log(), "Killing network...");
                self.kill();
                info!(self.ctx.log(), "Killed network.");
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

trait Routable {
    fn source(&self) -> &ActorPath;
    fn destination(&self) -> &ActorPath;
    fn to_serialised(self, buf: &mut BufferEncoder) -> Result<SerialisedFrame, SerError>;
    fn to_local(self) -> Result<NetMessage, SerError>;
}

impl Routable for NetMessage {
    fn source(&self) -> &ActorPath {
        &self.sender
    }

    fn destination(&self) -> &ActorPath {
        &self.receiver
    }

    fn to_serialised(self, buf: &mut BufferEncoder) -> Result<SerialisedFrame, SerError> {
        crate::ser_helpers::embed_msg(self, buf).map(|chunk| SerialisedFrame::Chunk(chunk))
    }

    fn to_local(self) -> Result<NetMessage, SerError> {
        Ok(self)
    }
}
impl Routable for (ActorPath, ActorPath, DispatchData) {
    fn source(&self) -> &ActorPath {
        &self.0
    }

    fn destination(&self) -> &ActorPath {
        &self.1
    }

    fn to_serialised(self, buf: &mut BufferEncoder) -> Result<SerialisedFrame, SerError> {
        self.2.to_serialised(self.0, self.1, buf)
    }

    fn to_local(self) -> Result<NetMessage, SerError> {
        self.2.to_local(self.0, self.1)
    }
}

#[cfg(test)]
mod dispatch_tests {
    use super::{super::*, *};
    use crate::prelude::Any;
    use bytes::{Buf, BufMut};
    use std::{thread, time::Duration};

    #[test]
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
        assert!(false, "System should not start correctly!");
        println!("KompactSystem started just fine.");
        let named_path = ActorPath::Named(NamedPath::with_system(
            system.system_path(),
            vec!["test".into()],
        ));
        println!("Got path: {}", named_path);
    }
    /*
    #[test]
    fn tokio_cleanup() {
        use tokio::net::TcpListener;

        let addr = "127.0.0.1:0"
            .parse::<SocketAddr>()
            .expect("Could not parse address!");
        let listener = TcpListener::bind(&addr).expect("Could not bind socket!");
        let actual_addr = listener.local_addr().expect("Could not get real address!");
        println!("Bound on {}", actual_addr);
        drop(listener);
        let listener2 = TcpListener::bind(&actual_addr).expect("Could not re-bind socket!");
        let actual_addr2 = listener2.local_addr().expect("Could not get real address!");
        println!("Bound again on {}", actual_addr2);
        assert_eq!(actual_addr, actual_addr2);
    } */

    #[test]
    fn network_cleanup() {
        let mut cfg = KompactConfig::new();
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
        let mut cfg2 = KompactConfig::new();
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
        let mut cfg = KompactConfig::new();
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

        let mut cfg2 = KompactConfig::new();
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
        let mut cfg = KompactConfig::new();
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
    fn named_registration() {
        const ACTOR_NAME: &str = "ponger";

        let mut cfg = KompactConfig::new();
        cfg.system_components(DeadletterBox::new, NetworkConfig::default().build());
        let system = cfg.build().expect("KompactSystem");
        let ponger = system.create(PongerAct::new);
        system.start(&ponger);

        let _res = system.register_by_alias(&ponger, ACTOR_NAME).wait_expect(
            Duration::from_millis(1000),
            "Single registration with unique alias should succeed.",
        );

        let res = system
            .register_by_alias(&ponger, ACTOR_NAME)
            .wait_timeout(Duration::from_millis(1000))
            .expect("Registration never completed.");

        assert_eq!(
            res,
            Err(RegistrationError::DuplicateEntry),
            "Duplicate alias registration should fail."
        );

        system
            .kill_notify(ponger)
            .wait_timeout(Duration::from_millis(1000))
            .expect("Ponger did not die");
        thread::sleep(Duration::from_millis(1000));

        system
            .shutdown()
            .expect("Kompact didn't shut down properly");
    }

    #[test]
    /// Sets up two KompactSystems with 2x Pingers and Pongers. One Ponger is registered by UUID,
    /// the other by a custom name. One Pinger communicates with the UUID-registered Ponger,
    /// the other with the named Ponger. Both sets are expected to exchange PING_COUNT ping-pong
    /// messages.
    fn remote_delivery_to_registered_actors_eager() {
        let (system, remote) = {
            let system = || {
                let mut cfg = KompactConfig::new();
                cfg.system_components(DeadletterBox::new, NetworkConfig::default().build());
                cfg.build().expect("KompactSystem")
            };
            (system(), system())
        };
        let (ponger_unique, pouf) = remote.create_and_register(PongerAct::new_eager);
        let (ponger_named, ponf) = remote.create_and_register(PongerAct::new_eager);
        let poaf = remote.register_by_alias(&ponger_named, "custom_name");

        pouf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
        ponf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
        poaf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");

        let named_path = ActorPath::Named(NamedPath::with_system(
            remote.system_path(),
            vec!["custom_name".into()],
        ));

        let unique_path = ActorPath::Unique(UniquePath::with_system(
            remote.system_path(),
            ponger_unique.id().clone(),
        ));

        let (pinger_unique, piuf) =
            system.create_and_register(move || PingerAct::new_eager(unique_path));
        let (pinger_named, pinf) =
            system.create_and_register(move || PingerAct::new_eager(named_path));

        piuf.wait_expect(Duration::from_millis(1000), "Pinger failed to register!");
        pinf.wait_expect(Duration::from_millis(1000), "Pinger failed to register!");

        remote.start(&ponger_unique);
        remote.start(&ponger_named);
        system.start(&pinger_unique);
        system.start(&pinger_named);

        // TODO maybe we could do this a bit more reliable?
        thread::sleep(Duration::from_millis(7000));

        let pingfu = system.stop_notify(&pinger_unique);
        let pingfn = system.stop_notify(&pinger_named);
        let pongfu = remote.kill_notify(ponger_unique);
        let pongfn = remote.kill_notify(ponger_named);

        pingfu
            .wait_timeout(Duration::from_millis(1000))
            .expect("Pinger never stopped!");
        pongfu
            .wait_timeout(Duration::from_millis(1000))
            .expect("Ponger never died!");
        pingfn
            .wait_timeout(Duration::from_millis(1000))
            .expect("Pinger never stopped!");
        pongfn
            .wait_timeout(Duration::from_millis(1000))
            .expect("Ponger never died!");
        pinger_unique.on_definition(|c| {
            println!("pinger uniq count: {}", c.count);
            assert_eq!(c.count, PING_COUNT);
        });
        pinger_named.on_definition(|c| {
            println!("pinger named count: {}", c.count);
            assert_eq!(c.count, PING_COUNT);
        });

        system
            .shutdown()
            .expect("Kompact didn't shut down properly");
    }

    #[test]
    /// Sets up two KompactSystems with 2x Pingers and Pongers. One Ponger is registered by UUID,
    /// the other by a custom name. One Pinger communicates with the UUID-registered Ponger,
    /// the other with the named Ponger. Both sets are expected to exchange PING_COUNT ping-pong
    /// messages.
    fn remote_delivery_to_registered_actors_lazy() {
        let (system, remote) = {
            let system = || {
                let mut cfg = KompactConfig::new();
                cfg.system_components(DeadletterBox::new, NetworkConfig::default().build());
                cfg.build().expect("KompactSystem")
            };
            (system(), system())
        };
        let (ponger_unique, pouf) = remote.create_and_register(PongerAct::new);
        let (ponger_named, ponf) = remote.create_and_register(PongerAct::new);
        let poaf = remote.register_by_alias(&ponger_named, "custom_name");

        pouf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
        ponf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
        poaf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");

        let named_path = ActorPath::Named(NamedPath::with_system(
            remote.system_path(),
            vec!["custom_name".into()],
        ));

        let unique_path = ActorPath::Unique(UniquePath::with_system(
            remote.system_path(),
            ponger_unique.id().clone(),
        ));

        let (pinger_unique, piuf) = system.create_and_register(move || PingerAct::new(unique_path));
        let (pinger_named, pinf) = system.create_and_register(move || PingerAct::new(named_path));

        piuf.wait_expect(Duration::from_millis(1000), "Pinger failed to register!");
        pinf.wait_expect(Duration::from_millis(1000), "Pinger failed to register!");

        remote.start(&ponger_unique);
        remote.start(&ponger_named);
        system.start(&pinger_unique);
        system.start(&pinger_named);

        // TODO maybe we could do this a bit more reliable?
        thread::sleep(Duration::from_millis(7000));

        let pingfu = system.stop_notify(&pinger_unique);
        let pingfn = system.stop_notify(&pinger_named);
        let pongfu = remote.kill_notify(ponger_unique);
        let pongfn = remote.kill_notify(ponger_named);

        pingfu
            .wait_timeout(Duration::from_millis(1000))
            .expect("Pinger never stopped!");
        pongfu
            .wait_timeout(Duration::from_millis(1000))
            .expect("Ponger never died!");
        pingfn
            .wait_timeout(Duration::from_millis(1000))
            .expect("Pinger never stopped!");
        pongfn
            .wait_timeout(Duration::from_millis(1000))
            .expect("Ponger never died!");
        pinger_unique.on_definition(|c| {
            println!("pinger uniq count: {}", c.count);
            assert_eq!(c.count, PING_COUNT);
        });
        pinger_named.on_definition(|c| {
            println!("pinger named count: {}", c.count);
            assert_eq!(c.count, PING_COUNT);
        });

        system
            .shutdown()
            .expect("Kompact didn't shut down properly");
    }

    #[test]
    /// Sets up two KompactSystems 1 and 2a, with Named paths. It first spawns a pinger-ponger couple
    /// The ping pong process completes, it shuts down system2a, spawns a new pinger on system1
    /// and finally boots a new system 2b with an identical networkconfig and named actor registration
    /// The final ping pong round should then complete as system1 automatically reconnects to system2 and
    /// transfers the enqueued messages.
    #[ignore]
    fn remote_lost_and_continued_connection() {
        let system1 = || {
            let mut cfg = KompactConfig::new();
            cfg.system_components(
                DeadletterBox::new,
                NetworkConfig::new(SocketAddr::new("127.0.0.1".parse().unwrap(), 9111)).build(),
            );
            cfg.build().expect("KompactSystem")
        };
        let system2 = || {
            let mut cfg = KompactConfig::new();
            cfg.system_components(
                DeadletterBox::new,
                NetworkConfig::new(SocketAddr::new("127.0.0.1".parse().unwrap(), 9112)).build(),
            );
            cfg.build().expect("KompactSystem")
        };

        // Set-up system2a
        let system2a = system2();
        //let (ponger_unique, pouf) = remote.create_and_register(PongerAct::new);
        let (ponger_named, ponf) = system2a.create_and_register(PongerAct::new);
        let poaf = system2a.register_by_alias(&ponger_named, "custom_name");
        ponf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
        poaf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
        let named_path = ActorPath::Named(NamedPath::with_system(
            system2a.system_path(),
            vec!["custom_name".into()],
        ));
        let named_path_clone = named_path.clone();
        // Set-up system1
        let system1 = system1();
        let (pinger_named, pinf) =
            system1.create_and_register(move || PingerAct::new(named_path_clone));
        pinf.wait_expect(Duration::from_millis(1000), "Pinger failed to register!");

        system2a.start(&ponger_named);
        system1.start(&pinger_named);

        // Wait for the pingpong
        thread::sleep(Duration::from_millis(2000));

        // Assert that things are going as we expect
        let pongfn = system2a.kill_notify(ponger_named);
        pongfn
            .wait_timeout(Duration::from_millis(1000))
            .expect("Ponger never died!");
        pinger_named.on_definition(|c| {
            println!("pinger named count: {}", c.count);
            assert_eq!(c.count, PING_COUNT);
        });
        // We now kill system2
        system2a.shutdown().ok();
        // Start a new pinger on system1
        let (pinger_named2, pinf2) =
            system1.create_and_register(move || PingerAct::new(named_path));
        pinf2.wait_expect(Duration::from_millis(1000), "Pinger failed to register!");
        system1.start(&pinger_named2);
        // Wait for it to send its pings, system1 should recognize the remote address
        thread::sleep(Duration::from_millis(1000));
        // Assert that things are going as they should be, ping count has not increased
        pinger_named2.on_definition(|c| {
            println!("pinger named count: {}", c.count);
            assert_eq!(c.count, 0);
        });
        // Start up system2b
        println!("Setting up system2b");
        let system2b = system2();
        let (ponger_named, ponf) = system2b.create_and_register(PongerAct::new);
        let poaf = system2b.register_by_alias(&ponger_named, "custom_name");
        ponf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
        poaf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
        println!("Starting actor on system2b");
        system2b.start(&ponger_named);

        // We give the connection plenty of time to re-establish and transfer it's old queue
        println!("Final sleep");
        thread::sleep(Duration::from_millis(5000));
        // Final assertion, did our systems re-connect without lost messages?
        println!("Asserting");
        pinger_named2.on_definition(|c| {
            println!("pinger named count: {}", c.count);
            assert_eq!(c.count, PING_COUNT);
        });
        println!("Shutting down");
        system1
            .shutdown()
            .expect("Kompact didn't shut down properly");
        system2b
            .shutdown()
            .expect("Kompact didn't shut down properly");
    }

    #[test]
    /// Identical with `remote_lost_and_continued_connection` up to the final sleep time and assertion
    /// system1 times out in its reconnection attempts and drops the enqueued buffers.
    /// After indirectly asserting that the queue was dropped we start up a new pinger, and assert that it succeeds.
    #[ignore]
    fn remote_lost_and_dropped_connection() {
        let system1 = || {
            let mut cfg = KompactConfig::new();
            cfg.system_components(
                DeadletterBox::new,
                NetworkConfig::new(SocketAddr::new("127.0.0.1".parse().unwrap(), 9211)).build(),
            );
            cfg.build().expect("KompactSystem")
        };
        let system2 = || {
            let mut cfg = KompactConfig::new();
            cfg.system_components(
                DeadletterBox::new,
                NetworkConfig::new(SocketAddr::new("127.0.0.1".parse().unwrap(), 9212)).build(),
            );
            cfg.build().expect("KompactSystem")
        };

        // Set-up system2a
        let system2a = system2();
        //let (ponger_unique, pouf) = remote.create_and_register(PongerAct::new);
        let (ponger_named, ponf) = system2a.create_and_register(PongerAct::new);
        let poaf = system2a.register_by_alias(&ponger_named, "custom_name");
        ponf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
        poaf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
        let named_path = ActorPath::Named(NamedPath::with_system(
            system2a.system_path(),
            vec!["custom_name".into()],
        ));
        let named_path_clone = named_path.clone();
        let named_path_clone2 = named_path.clone();
        // Set-up system1
        let system1 = system1();
        let (pinger_named, pinf) =
            system1.create_and_register(move || PingerAct::new(named_path_clone));
        pinf.wait_expect(Duration::from_millis(1000), "Pinger failed to register!");

        system2a.start(&ponger_named);
        system1.start(&pinger_named);

        // Wait for the pingpong
        thread::sleep(Duration::from_millis(2000));

        // Assert that things are going as we expect
        let pongfn = system2a.kill_notify(ponger_named);
        pongfn
            .wait_timeout(Duration::from_millis(1000))
            .expect("Ponger never died!");
        pinger_named.on_definition(|c| {
            println!("pinger named count: {}", c.count);
            assert_eq!(c.count, PING_COUNT);
        });
        // We now kill system2
        system2a.shutdown().ok();
        // Start a new pinger on system1
        let (pinger_named2, pinf2) =
            system1.create_and_register(move || PingerAct::new(named_path));
        pinf2.wait_expect(Duration::from_millis(1000), "Pinger failed to register!");
        system1.start(&pinger_named2);
        // Wait for it to send its pings, system1 should recognize the remote address
        thread::sleep(Duration::from_millis(1000));
        // Assert that things are going as they should be, ping count has not increased
        pinger_named2.on_definition(|c| {
            println!("pinger named count: {}", c.count);
            assert_eq!(c.count, 0);
        });
        // Sleep long-enough that the remote connection will be dropped with its queue
        thread::sleep(Duration::from_millis(
            (MAX_RETRY_ATTEMPTS + 2) as u64 * RETRY_CONNECTIONS_INTERVAL,
        ));
        // Start up system2b
        println!("Setting up system2b");
        let system2b = system2();
        let (ponger_named, ponf) = system2b.create_and_register(PongerAct::new);
        let poaf = system2b.register_by_alias(&ponger_named, "custom_name");
        ponf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
        poaf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
        println!("Starting actor on system2b");
        system2b.start(&ponger_named);

        // We give the connection plenty of time to re-establish and transfer it's old queue
        thread::sleep(Duration::from_millis(5000));
        // Final assertion, did our systems re-connect without lost messages?
        pinger_named2.on_definition(|c| {
            println!("pinger named count: {}", c.count);
            assert_eq!(c.count, 0);
        });

        // This one should now succeed
        let (pinger_named2, pinf2) =
            system1.create_and_register(move || PingerAct::new(named_path_clone2));
        pinf2.wait_expect(Duration::from_millis(1000), "Pinger failed to register!");
        system1.start(&pinger_named2);
        // Wait for it to send its pings, system1 should recognize the remote address
        thread::sleep(Duration::from_millis(1000));
        // Assert that things are going as they should be
        pinger_named2.on_definition(|c| {
            println!("pinger named count: {}", c.count);
            assert_eq!(c.count, PING_COUNT);
        });

        system1
            .shutdown()
            .expect("Kompact didn't shut down properly");
        system2b
            .shutdown()
            .expect("Kompact didn't shut down properly");
    }

    const PING_COUNT: u64 = 10;

    #[test]
    fn local_delivery() {
        let mut cfg = KompactConfig::new();
        cfg.system_components(DeadletterBox::new, NetworkConfig::default().build());
        let system = cfg.build().expect("KompactSystem");

        let (ponger, pof) = system.create_and_register(PongerAct::new);
        // Construct ActorPath with system's `proto` field explicitly set to LOCAL
        let mut ponger_path = system.actor_path_for(&ponger);
        ponger_path.set_transport(Transport::LOCAL);
        let (pinger, pif) = system.create_and_register(move || PingerAct::new(ponger_path));

        pof.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
        pif.wait_expect(Duration::from_millis(1000), "Pinger failed to register!");

        system.start(&ponger);
        system.start(&pinger);

        // TODO no sleeps!
        thread::sleep(Duration::from_millis(1000));

        let pingf = system.stop_notify(&pinger);
        let pongf = system.kill_notify(ponger);
        pingf
            .wait_timeout(Duration::from_millis(1000))
            .expect("Pinger never stopped!");
        pongf
            .wait_timeout(Duration::from_millis(1000))
            .expect("Ponger never died!");
        pinger.on_definition(|c| {
            assert_eq!(c.count, PING_COUNT);
        });

        system
            .shutdown()
            .expect("Kompact didn't shut down properly");
    }

    #[test]
    fn local_forwarding() {
        let mut cfg = KompactConfig::new();
        cfg.system_components(DeadletterBox::new, NetworkConfig::default().build());
        let system = cfg.build().expect("KompactSystem");

        let (ponger, pof) = system.create_and_register(PongerAct::new);
        // Construct ActorPath with system's `proto` field explicitly set to LOCAL
        let mut ponger_path =
            pof.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
        ponger_path.set_transport(Transport::LOCAL);

        let (forwarder, fof) = system.create_and_register(move || ForwarderAct::new(ponger_path));
        let mut forwarder_path =
            fof.wait_expect(Duration::from_millis(1000), "Forwarder failed to register!");
        forwarder_path.set_transport(Transport::LOCAL);

        let (pinger, pif) = system.create_and_register(move || PingerAct::new(forwarder_path));
        let _pinger_path =
            pif.wait_expect(Duration::from_millis(1000), "Pinger failed to register!");

        system.start(&ponger);
        system.start(&forwarder);
        system.start(&pinger);

        // TODO no sleeps!
        thread::sleep(Duration::from_millis(1000));

        let pingf = system.kill_notify(pinger.clone()); // hold on to this ref so we can check count later
        let forwf = system.kill_notify(forwarder);
        let pongf = system.kill_notify(ponger);
        pingf
            .wait_timeout(Duration::from_millis(1000))
            .expect("Pinger never stopped!");
        forwf
            .wait_timeout(Duration::from_millis(1000))
            .expect("Forwarder never stopped!");
        pongf
            .wait_timeout(Duration::from_millis(1000))
            .expect("Ponger never died!");
        pinger.on_definition(|c| {
            assert_eq!(c.count, PING_COUNT);
        });

        system
            .shutdown()
            .expect("Kompact didn't shut down properly");
    }

    #[test]
    fn local_forwarding_eager() {
        let mut cfg = KompactConfig::new();
        cfg.system_components(DeadletterBox::new, NetworkConfig::default().build());
        let system = cfg.build().expect("KompactSystem");

        let (ponger, pof) = system.create_and_register(PongerAct::new);
        // Construct ActorPath with system's `proto` field explicitly set to LOCAL
        let mut ponger_path =
            pof.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
        ponger_path.set_transport(Transport::LOCAL);

        let (forwarder, fof) = system.create_and_register(move || ForwarderAct::new(ponger_path));
        let mut forwarder_path =
            fof.wait_expect(Duration::from_millis(1000), "Forwarder failed to register!");
        forwarder_path.set_transport(Transport::LOCAL);

        let (pinger, pif) =
            system.create_and_register(move || PingerAct::new_eager(forwarder_path));
        let _pinger_path =
            pif.wait_expect(Duration::from_millis(1000), "Pinger failed to register!");

        system.start(&ponger);
        system.start(&forwarder);
        system.start(&pinger);

        // TODO no sleeps!
        thread::sleep(Duration::from_millis(1000));

        let pingf = system.kill_notify(pinger.clone()); // hold on to this ref so we can check count later
        let forwf = system.kill_notify(forwarder);
        let pongf = system.kill_notify(ponger);
        pingf
            .wait_timeout(Duration::from_millis(1000))
            .expect("Pinger never stopped!");
        forwf
            .wait_timeout(Duration::from_millis(1000))
            .expect("Forwarder never stopped!");
        pongf
            .wait_timeout(Duration::from_millis(1000))
            .expect("Ponger never died!");
        pinger.on_definition(|c| {
            assert_eq!(c.count, PING_COUNT);
        });

        system
            .shutdown()
            .expect("Kompact didn't shut down properly");
    }

    #[test]
    fn remote_forwarding_unique() {
        let (system1, system2, system3) = {
            let system = || {
                let mut cfg = KompactConfig::new();
                cfg.system_components(DeadletterBox::new, NetworkConfig::default().build());
                cfg.build().expect("KompactSystem")
            };
            (system(), system(), system())
        };

        let (ponger, pof) = system1.create_and_register(PongerAct::new);
        let ponger_path =
            pof.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");

        let (forwarder, fof) = system2.create_and_register(move || ForwarderAct::new(ponger_path));
        let forwarder_path =
            fof.wait_expect(Duration::from_millis(1000), "Forwarder failed to register!");

        let (pinger, pif) = system3.create_and_register(move || PingerAct::new(forwarder_path));
        let _pinger_path =
            pif.wait_expect(Duration::from_millis(1000), "Pinger failed to register!");

        system1.start(&ponger);
        system2.start(&forwarder);
        system3.start(&pinger);

        // TODO no sleeps!
        thread::sleep(Duration::from_millis(1000));

        let pingf = system3.kill_notify(pinger.clone()); // hold on to this ref so we can check count later
        let forwf = system2.kill_notify(forwarder);
        let pongf = system1.kill_notify(ponger);
        pingf
            .wait_timeout(Duration::from_millis(1000))
            .expect("Pinger never died!");
        forwf
            .wait_timeout(Duration::from_millis(1000))
            .expect("Forwarder never died!");
        pongf
            .wait_timeout(Duration::from_millis(1000))
            .expect("Ponger never died!");

        pinger.on_definition(|c| {
            assert_eq!(c.count, PING_COUNT);
        });

        system1
            .shutdown()
            .expect("Kompact didn't shut down properly");
        system2
            .shutdown()
            .expect("Kompact didn't shut down properly");
        system3
            .shutdown()
            .expect("Kompact didn't shut down properly");
    }

    #[test]
    fn remote_forwarding_unique_two_systems() {
        let (system1, system2) = {
            let system = || {
                let mut cfg = KompactConfig::new();
                cfg.system_components(DeadletterBox::new, NetworkConfig::default().build());
                cfg.build().expect("KompactSystem")
            };
            (system(), system())
        };

        let (ponger, pof) = system1.create_and_register(PongerAct::new);
        let ponger_path =
            pof.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");

        let (forwarder, fof) = system2.create_and_register(move || ForwarderAct::new(ponger_path));
        let forwarder_path =
            fof.wait_expect(Duration::from_millis(1000), "Forwarder failed to register!");

        let (pinger, pif) = system1.create_and_register(move || PingerAct::new(forwarder_path));
        let _pinger_path =
            pif.wait_expect(Duration::from_millis(1000), "Pinger failed to register!");

        system1.start(&ponger);
        system2.start(&forwarder);
        system1.start(&pinger);

        // TODO no sleeps!
        thread::sleep(Duration::from_millis(1000));

        let pingf = system1.kill_notify(pinger.clone()); // hold on to this ref so we can check count later
        let forwf = system2.kill_notify(forwarder);
        let pongf = system1.kill_notify(ponger);
        pingf
            .wait_timeout(Duration::from_millis(1000))
            .expect("Pinger never died!");
        forwf
            .wait_timeout(Duration::from_millis(1000))
            .expect("Forwarder never died!");
        pongf
            .wait_timeout(Duration::from_millis(1000))
            .expect("Ponger never died!");

        pinger.on_definition(|c| {
            assert_eq!(c.count, PING_COUNT);
        });

        system1
            .shutdown()
            .expect("Kompact didn't shut down properly");
        system2
            .shutdown()
            .expect("Kompact didn't shut down properly");
    }

    #[test]
    fn remote_forwarding_named() {
        let (system1, system2, system3) = {
            let system = || {
                let mut cfg = KompactConfig::new();
                cfg.system_components(DeadletterBox::new, NetworkConfig::default().build());
                cfg.build().expect("KompactSystem")
            };
            (system(), system(), system())
        };

        let (ponger, _pof) = system1.create_and_register(PongerAct::new);
        let pnf = system1.register_by_alias(&ponger, "ponger");
        let ponger_path =
            pnf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");

        let (forwarder, _fof) = system2.create_and_register(move || ForwarderAct::new(ponger_path));
        let fnf = system2.register_by_alias(&forwarder, "forwarder");
        let forwarder_path =
            fnf.wait_expect(Duration::from_millis(1000), "Forwarder failed to register!");

        let (pinger, pif) = system3.create_and_register(move || PingerAct::new(forwarder_path));
        let _pinger_path =
            pif.wait_expect(Duration::from_millis(1000), "Pinger failed to register!");

        system1.start(&ponger);
        system2.start(&forwarder);
        system3.start(&pinger);

        // TODO no sleeps!
        thread::sleep(Duration::from_millis(1000));

        let pingf = system3.kill_notify(pinger.clone()); // hold on to this ref so we can check count later
        let forwf = system2.kill_notify(forwarder);
        let pongf = system1.kill_notify(ponger);
        pingf
            .wait_timeout(Duration::from_millis(1000))
            .expect("Pinger never died!");
        forwf
            .wait_timeout(Duration::from_millis(1000))
            .expect("Forwarder never died!");
        pongf
            .wait_timeout(Duration::from_millis(1000))
            .expect("Ponger never died!");

        pinger.on_definition(|c| {
            assert_eq!(c.count, PING_COUNT);
        });

        system1
            .shutdown()
            .expect("Kompact didn't shut down properly");
        system2
            .shutdown()
            .expect("Kompact didn't shut down properly");
        system3
            .shutdown()
            .expect("Kompact didn't shut down properly");
    }

    #[derive(Debug, Clone)]
    struct PingMsg {
        i: u64,
    }

    #[derive(Debug, Clone)]
    struct PongMsg {
        i: u64,
    }

    struct PingPongSer;

    impl PingPongSer {
        const SID: SerId = 42;
    }

    const PING_ID: i8 = 1;
    const PONG_ID: i8 = 2;

    impl Serialiser<PingMsg> for PingPongSer {
        fn ser_id(&self) -> SerId {
            Self::SID
        }

        fn size_hint(&self) -> Option<usize> {
            Some(9)
        }

        fn serialise(&self, v: &PingMsg, buf: &mut dyn BufMut) -> Result<(), SerError> {
            buf.put_i8(PING_ID);
            buf.put_u64(v.i);
            Result::Ok(())
        }
    }

    impl Serialisable for PingMsg {
        fn ser_id(&self) -> SerId {
            42
        }

        fn size_hint(&self) -> Option<usize> {
            Some(9)
        }

        fn serialise(&self, buf: &mut dyn BufMut) -> Result<(), SerError> {
            buf.put_i8(PING_ID);
            buf.put_u64(self.i);
            Result::Ok(())
        }

        fn local(self: Box<Self>) -> Result<Box<dyn Any + Send>, Box<dyn Serialisable>> {
            Ok(self)
        }
    }

    impl Serialisable for PongMsg {
        fn ser_id(&self) -> SerId {
            42
        }

        fn size_hint(&self) -> Option<usize> {
            Some(9)
        }

        fn serialise(&self, buf: &mut dyn BufMut) -> Result<(), SerError> {
            buf.put_i8(PONG_ID);
            buf.put_u64(self.i);
            Result::Ok(())
        }

        fn local(self: Box<Self>) -> Result<Box<dyn Any + Send>, Box<dyn Serialisable>> {
            Ok(self)
        }
    }

    impl Serialiser<PongMsg> for PingPongSer {
        fn ser_id(&self) -> SerId {
            Self::SID
        }

        fn size_hint(&self) -> Option<usize> {
            Some(9)
        }

        fn serialise(&self, v: &PongMsg, buf: &mut dyn BufMut) -> Result<(), SerError> {
            buf.put_i8(PONG_ID);
            buf.put_u64(v.i);
            Result::Ok(())
        }
    }

    impl Deserialiser<PingMsg> for PingPongSer {
        const SER_ID: SerId = Self::SID;

        fn deserialise(buf: &mut dyn Buf) -> Result<PingMsg, SerError> {
            if buf.remaining() < 9 {
                return Err(SerError::InvalidData(format!(
                    "Serialised typed has 9bytes but only {}bytes remain in buffer.",
                    buf.remaining()
                )));
            }
            match buf.get_i8() {
                PING_ID => {
                    let i = buf.get_u64();
                    Ok(PingMsg { i })
                }
                PONG_ID => Err(SerError::InvalidType(
                    "Found PongMsg, but expected PingMsg.".into(),
                )),
                id => Err(SerError::InvalidType(
                    format!("Found unknown id {}, but expected PingMsg.", id).into(),
                )),
            }
        }
    }

    impl Deserialiser<PongMsg> for PingPongSer {
        const SER_ID: SerId = Self::SID;

        fn deserialise(buf: &mut dyn Buf) -> Result<PongMsg, SerError> {
            if buf.remaining() < 9 {
                return Err(SerError::InvalidData(format!(
                    "Serialised typed has 9bytes but only {}bytes remain in buffer.",
                    buf.remaining()
                )));
            }
            match buf.get_i8() {
                PONG_ID => {
                    let i = buf.get_u64();
                    Ok(PongMsg { i })
                }
                PING_ID => Err(SerError::InvalidType(
                    "Found PingMsg, but expected PongMsg.".into(),
                )),
                id => Err(SerError::InvalidType(
                    format!("Found unknown id {}, but expected PingMsg.", id).into(),
                )),
            }
        }
    }

    #[derive(ComponentDefinition)]
    struct PingerAct {
        ctx: ComponentContext<PingerAct>,
        target: ActorPath,
        count: u64,
        eager: bool,
    }

    impl PingerAct {
        fn new(target: ActorPath) -> PingerAct {
            PingerAct {
                ctx: ComponentContext::uninitialised(),
                target,
                count: 0,
                eager: false,
            }
        }

        fn new_eager(target: ActorPath) -> PingerAct {
            PingerAct {
                ctx: ComponentContext::uninitialised(),
                target,
                count: 0,
                eager: true,
            }
        }
    }

    impl Provide<ControlPort> for PingerAct {
        fn handle(&mut self, event: ControlEvent) -> Handled {
            match event {
                ControlEvent::Start => {
                    info!(self.ctx.log(), "Starting");
                    if self.eager {
                        self.target
                            .tell_serialised(PingMsg { i: 0 }, self)
                            .expect("serialise");
                    } else {
                        self.target.tell(PingMsg { i: 0 }, self);
                    }
                }
                _ => (),
            }
            Handled::Ok
        }
    }

    impl Actor for PingerAct {
        type Message = Never;

        fn receive_local(&mut self, _pong: Self::Message) -> Handled {
            unimplemented!();
        }

        fn receive_network(&mut self, msg: NetMessage) -> Handled {
            match msg.try_deserialise::<PongMsg, PingPongSer>() {
                Ok(pong) => {
                    info!(self.ctx.log(), "Got msg {:?}", pong);
                    self.count += 1;
                    if self.count < PING_COUNT {
                        if self.eager {
                            self.target
                                .tell_serialised((PingMsg { i: pong.i + 1 }), self)
                                .expect("serialise");
                        } else {
                            self.target.tell((PingMsg { i: pong.i + 1 }), self);
                        }
                    }
                }
                Err(e) => error!(self.ctx.log(), "Error deserialising PongMsg: {:?}", e),
            }
            Handled::Ok
        }
    }

    #[derive(ComponentDefinition)]
    struct PongerAct {
        ctx: ComponentContext<PongerAct>,
        eager: bool,
    }

    impl PongerAct {
        fn new() -> PongerAct {
            PongerAct {
                ctx: ComponentContext::uninitialised(),
                eager: false,
            }
        }

        fn new_eager() -> PongerAct {
            PongerAct {
                ctx: ComponentContext::uninitialised(),
                eager: true,
            }
        }
    }

    impl Provide<ControlPort> for PongerAct {
        fn handle(&mut self, event: ControlEvent) -> Handled {
            match event {
                ControlEvent::Start => {
                    info!(self.ctx.log(), "Starting");
                }
                _ => (),
            }
            Handled::Ok
        }
    }

    impl Actor for PongerAct {
        type Message = Never;

        fn receive_local(&mut self, _ping: Self::Message) -> Handled {
            unimplemented!();
        }

        fn receive_network(&mut self, msg: NetMessage) -> Handled {
            let sender = msg.sender;
            match_deser! {msg.data; {
                ping: PingMsg [PingPongSer] => {
                    info!(self.ctx.log(), "Got msg {:?} from {}", ping, sender);
                    let pong = PongMsg { i: ping.i };
                    if self.eager {
                        sender
                            .tell_serialised(pong, self)
                            .expect("PongMsg should serialise");
                    } else {
                        sender.tell(pong, self);
                    }
                },
                !Err(e) => error!(self.ctx.log(), "Error deserialising PingMsg: {:?}", e),
            }}
            Handled::Ok
        }
    }

    #[derive(ComponentDefinition)]
    struct ForwarderAct {
        ctx: ComponentContext<Self>,
        forward_to: ActorPath,
    }
    impl ForwarderAct {
        fn new(forward_to: ActorPath) -> Self {
            ForwarderAct {
                ctx: ComponentContext::uninitialised(),
                forward_to,
            }
        }
    }
    ignore_control!(ForwarderAct);
    impl Actor for ForwarderAct {
        type Message = Never;

        fn receive_local(&mut self, _ping: Self::Message) -> Handled {
            unimplemented!();
        }

        fn receive_network(&mut self, msg: NetMessage) -> Handled {
            info!(
                self.ctx.log(),
                "Forwarding some msg from {} to {}", msg.sender, self.forward_to
            );
            self.forward_to.forward_with_original_sender(msg, self);
            Handled::Ok
        }
    }
}

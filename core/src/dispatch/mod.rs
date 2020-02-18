use super::*;

use crate::{
    actors::{Actor, ActorPath, Dispatcher, DynActorRef, SystemPath, Transport},
    component::{Component, ComponentContext, ExecuteResult, Provide},
    lifecycle::{ControlEvent, ControlPort},
};
use std::{net::SocketAddr, sync::Arc};

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
    },
    net::{events::NetworkEvent, ConnectionState},
};
use arc_swap::ArcSwap;
use futures::{self, Async, AsyncSink, Poll, StartSend};
//use std::collections::HashMap;
use fnv::FnvHashMap;
use std::{io::ErrorKind, time::Duration};
use crate::net::buffer::ChunkLease;

pub mod lookup;
pub mod queue_manager;

/// Configuration builder for network dispatcher.
#[derive(Clone, PartialEq, Debug)]
pub struct NetworkConfig {
    addr: SocketAddr,
    transport: Transport,
}

impl NetworkConfig {
    pub fn new(addr: SocketAddr) -> Self {
        NetworkConfig {
            addr,
            transport: Transport::TCP,
        }
    }

    /// Replace current socket addrss with `addr`.
    pub fn with_socket(mut self, addr: SocketAddr) -> Self {
        self.addr = addr;
        self
    }

    pub fn build(self) -> impl Fn(Promise<()>) -> NetworkDispatcher {
        move |notify_ready| NetworkDispatcher::with_config(self.clone(), notify_ready)
    }
}

/// Socket defaults to `127.0.0.1:0` (i.e. a random local port).
impl Default for NetworkConfig {
    fn default() -> Self {
        NetworkConfig {
            addr: "127.0.0.1:0".parse().unwrap(),
            transport: Transport::TCP,
        }
    }
}

/// Network-aware dispatcher for messages to remote actors.
#[derive(ComponentDefinition)]
pub struct NetworkDispatcher {
    ctx: ComponentContext<NetworkDispatcher>,
    /// Local map of connection statuses
    connections: FnvHashMap<SocketAddr, ConnectionState>,
    /// Network configuration for this dispatcher
    cfg: NetworkConfig,
    /// Shared lookup structure for mapping [ActorPath]s and [ActorRefs]
    lookup: Arc<ArcSwap<ActorStore>>,
    // Fields initialized at [ControlEvent::Start]; they require ComponentContextual awareness
    /// Bridge into asynchronous networking layer
    net_bridge: Option<net::Bridge>,
    /// Management for queuing Frames during network unavailability (conn. init. and MPSC unreadiness)
    queue_manager: Option<QueueManager>,
    /// Reaper which cleans up deregistered actor references in the actor lookup table
    reaper: lookup::gc::ActorRefReaper,
    notify_ready: Option<Promise<()>>,
}

// impl NetworkDispatcher
impl NetworkDispatcher {
    pub fn new(notify_ready: Promise<()>) -> Self {
        let config = NetworkConfig::default();
        NetworkDispatcher::with_config(config, notify_ready)
    }

    pub fn with_config(cfg: NetworkConfig, notify_ready: Promise<()>) -> Self {
        let lookup = Arc::new(ArcSwap::from(Arc::new(ActorStore::new())));
        let reaper = lookup::gc::ActorRefReaper::default();

        NetworkDispatcher {
            ctx: ComponentContext::new(),
            connections: FnvHashMap::default(),
            cfg,
            lookup,
            net_bridge: None,
            queue_manager: None,
            reaper,
            notify_ready: Some(notify_ready),
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
        let (mut bridge, addr) = net::Bridge::new(self.lookup.clone(), network_thread_logger, bridge_logger, self.cfg.addr.clone(), dispatcher.clone());

        bridge.set_dispatcher(dispatcher.clone());
        bridge.start()?;
        self.net_bridge = Some(bridge);
        /*
        if let Some(ref ex) = bridge.executor.as_ref() {
            use futures::{Future, Stream};
            ex.spawn(
                events
                    .map(|ev| DispatchEnvelope::Event(EventEnvelope::Network(ev)))
                    .forward(dispatcher)
                    .then(|_| Ok(())),
            );
        } else {
            return Err(net::NetworkBridgeErr::Other(
                "No executor found in network bridge; network events can not be handled"
                    .to_string(),
            ));
        } */
        let queue_manager = QueueManager::new();
        self.queue_manager = Some(queue_manager);
        Ok(())
    }

    fn stop(&mut self) -> () {
        self.do_stop(false)
    }

    fn kill(&mut self) -> () {
        self.do_stop(true)
    }

    fn do_stop(&mut self, _cleanup: bool) -> () {
        if let Some(manager) = self.queue_manager.take() {
            manager.stop();
        }
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
            target.schedule_reaper()
        });
    }

    fn on_event(&mut self, ev: EventEnvelope) {
        //println!("Dispatch on_event");
        match ev {
            EventEnvelope::Network(ev) => match ev {
                NetworkEvent::Connection(addr, conn_state) => {
                    //println!("Connection...");
                    self.on_conn_state(addr, conn_state)
                },
                NetworkEvent::Data(_) => {
                    //println!("Data _");
                    // TODO shouldn't be receiving these here, as they should be routed directly to the ActorRef
                    debug!(self.ctx().log(), "Received important data!");
                }
                _ => {
                    //println!("_ match");
                    // TODO shouldn't be receiving these here, as they should be routed directly to the ActorRef
                    debug!(self.ctx().log(), "Received important data!");
                }
            },
        }
    }

    fn on_conn_state(&mut self, addr: SocketAddr, mut state: ConnectionState) {
        use self::ConnectionState::*;
        //println!("on_conn_state for addr {}", &addr);
        match state {
            Connected(ref mut frame_sender) => {
                info!(
                    self.ctx().log(),
                    "registering newly connected conn at {:?}", addr
                );
                if let Some(ref mut qm) = self.queue_manager {
                    if qm.has_frame(&addr) {
                        // Drain as much as possible
                        while let Some(frame) = qm.pop_frame(&addr) {
                            if let Some(bridge) = &self.net_bridge {
                                //println!("Sending queued frame to newly established connection");
                                bridge.route(addr, frame);
                            }
                            /*if let Err(err) = frame_sender.unbounded_send(frame) {
                                // TODO the underlying channel has been dropped,
                                // indicating that the entire connection is, in fact, not Connected
                                qm.enqueue_frame(err.into_inner(), addr.clone());
                                break;
                            }*/
                        }
                    }
                }
            }
            Closed => {
                warn!(self.ctx().log(), "connection closed for {:?}", addr);
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
    }

    /// Forwards `msg` up to a local `dst` actor, if it exists.
    fn route_local(&mut self, src: ActorPath, dst: ActorPath, msg: DispatchData) {
        use crate::dispatch::lookup::ActorLookup;
        let lookup = self.lookup.lease();
        let ser_id = msg.ser_id();
        let actor_opt = lookup.get_by_actor_path(&dst);
        let netmsg = match msg.to_local(src, dst) {
            Ok(m) => m,
            Err(e) => {
                error!(
                    self.ctx.log(),
                    "Could not serialise a local message (ser_id = {}). Dropping. Error was: {:?}",
                    ser_id,
                    e
                );
                return;
            }
        };
        if let Some(ref actor) = actor_opt {
            actor.enqueue(netmsg);
        } else {
            error!(
                self.ctx.log(),
                "No local actor found at {:?}. Forwarding to DeadletterBox",
                netmsg.receiver(),
            );
            self.ctx.deadletter_ref().enqueue(MsgEnvelope::Net(netmsg));
        }
    }

    /// Routes the provided message to the destination, or queues the message until the connection
    /// is available.
    fn route_remote(&mut self, src: ActorPath, dst: ActorPath, msg: DispatchData) {
        use crate::net::frames::*;

        let addr = SocketAddr::new(dst.address().clone(), dst.port());
        /*
        match msg {
            DispatchData::Eager(serialized) => {

            }
            DispatchData::Lazy(serializable) => {}
        }
        */
        let serialized = {
            let ser_id = msg.ser_id();
            let mut payload = match msg.to_serialised(src, dst) {
                Ok(p) => p,
                Err(e) => {
                    error!(
                    self.ctx.log(),
                    "Could not serialise a remote message (ser_id = {}). Dropping. Error was: {:?}",
                    ser_id,
                    e
                );
                    return;
                }
            };
            payload
        };

        let state: &mut ConnectionState =
            self.connections.entry(addr).or_insert(ConnectionState::New);
        let next: Option<ConnectionState> = match *state {
            ConnectionState::New | ConnectionState::Closed => {
                //println!("route_remote found New or Closed state");
                debug!(
                    self.ctx.log(),
                    "No connection found; establishing and queuing frame"
                );
                self.queue_manager
                    .as_mut()
                    .map(|ref mut q| q.enqueue_frame(serialized, addr));

                if let Some(ref mut bridge) = self.net_bridge {
                    debug!(self.ctx.log(), "Establishing new connection to {:?}", addr);
                    bridge.connect(Transport::TCP, addr).unwrap();
                    Some(ConnectionState::Initializing)
                } else {
                    error!(self.ctx.log(), "No network bridge found; dropping message");
                    Some(ConnectionState::Closed)
                }
            }
            ConnectionState::Connected(_) => {
                //println!("route_remote found Connected state");
                if let Some(ref mut qm) = self.queue_manager {
                    if qm.has_frame(&addr) {
                        qm.enqueue_frame(serialized, addr.clone());

                        if let Some(bridge) = &self.net_bridge {
                            while let Some(frame) = qm.pop_frame(&addr) {
                                bridge.route(addr, frame);
                            }
                        }
                        None
                    } else {
                        // Send frame
                        if let Some(bridge) = &self.net_bridge {
                            bridge.route(addr, serialized);
                        }
                        None
                    }
                } else {
                    // No queue manager available! Should we even allow this state?
                    None
                }
            }
            ConnectionState::Initializing => {
                debug!(self.ctx.log(), "Connection is initializing; queuing frame");
                self.queue_manager
                    .as_mut()
                    .map(|ref mut q| q.enqueue_frame(serialized, addr));
                None
            }
            _ => None,
        };

        if let Some(next) = next {
            *state = next;
        }

    }

    fn resolve_path(&mut self, resolvable: &PathResolvable) -> ActorPath {
        match resolvable {
            PathResolvable::Path(actor_path) => actor_path.clone(),
            PathResolvable::Alias(alias) => ActorPath::Named(NamedPath::with_system(
                self.system_path(),
                vec![alias.clone()],
            )),
            PathResolvable::ActorId(uuid) => {
                ActorPath::Unique(UniquePath::with_system(self.system_path(), uuid.clone()))
            }
            PathResolvable::System => self.actor_path(),
        }
    }

    /// Forwards `msg` to destination described by `dst`, routing it across the network
    /// if needed.
    fn route(&mut self, src: PathResolvable, dst_path: ActorPath, msg: DispatchData) {
        let mut src_path = self.resolve_path(&src);
        //println!("*** Dispatch route {:?}", dst_path.system().protocol());
        if src_path.system() == dst_path.system() {
            //println!("*** dst system == src system");
            self.route_local(src_path, dst_path, msg);
            return;
        }
        let proto = {
            let dst_sys = dst_path.system();
            SystemField::protocol(dst_sys)
        };
        match proto {
            Transport::LOCAL => {
                self.route_local(src_path, dst_path, msg);
            }
            Transport::TCP => {
                self.route_remote(src_path, dst_path, msg);
            }
            Transport::UDP => {
                error!(self.ctx.log(), "UDP routing is not supported.");
            }
        }
    }

    fn actor_path(&mut self) -> ActorPath {
        let uuid = self.ctx.id();
        ActorPath::Unique(UniquePath::with_system(self.system_path(), uuid.clone()))
    }
}

impl Actor for NetworkDispatcher {
    type Message = DispatchEnvelope;

    fn receive_local(&mut self, msg: Self::Message) -> () {
        match msg {
            DispatchEnvelope::Msg { src, dst, msg } => {
                // Look up destination (local or remote), then route or err
                self.route(src, dst, msg);
            }
            DispatchEnvelope::Registration(reg) => {
                use lookup::ActorLookup;

                match reg {
                    RegistrationEnvelope::Register(actor, path, promise) => {
                        let ap = self.resolve_path(&path);
                        let lease = self.lookup.lease();
                        let res = if lease.contains(&path) {
                            warn!(self.ctx.log(), "Detected duplicate path during registration. The path will not be re-registered");
                            drop(lease);
                            Err(RegistrationError::DuplicateEntry)
                        } else {
                            drop(lease);
                            self.lookup.rcu(move |current| {
                                let mut next = (*current).clone();
                                next.insert(actor.clone(), path.clone());
                                Arc::new(next)
                            });
                            Ok(ap)
                        };

                        if res.is_ok() {
                            if !self.reaper.is_scheduled() {
                                self.schedule_reaper();
                            }
                        }

                        if let Some(promise) = promise {
                            promise.fulfill(res).unwrap_or_else(|e| {
                                error!(self.ctx.log(), "Could not notify listeners: {:?}", e)
                            });
                        }
                    }
                }
            }
            DispatchEnvelope::Event(ev) => self.on_event(ev),
        }
    }

    fn receive_network(&mut self, msg: NetMessage) -> () {
        warn!(self.ctx.log(), "Received network message: {:?}", msg,);
    }
}

impl Dispatcher for NetworkDispatcher {
    /// Generates a [SystemPath](kompact::actors) from this dispatcher's configuration
    /// This is only possible after the socket is bound and will panic if attempted earlier!
    fn system_path(&mut self) -> SystemPath {
        // TODO get protocol from configuration
        let bound_addr = match self.net_bridge {
            Some(ref net_bridge) => net_bridge.local_addr().clone().expect("If net bridge is ready, port should be as well!"),
            None => panic!("You must wait until the socket is bound before attempting to create a system path!"),
        };
        SystemPath::new(self.cfg.transport, bound_addr.ip(), bound_addr.port())
    }
}

impl Provide<ControlPort> for NetworkDispatcher {
    fn handle(&mut self, event: ControlEvent) {
        match event {
            ControlEvent::Start => {
                info!(self.ctx.log(), "Starting network...");
                let res = self.start(); //.expect("Could not create NetworkDispatcher!");
                match res {
                    Ok(_) => {
                        info!(self.ctx.log(), "Started network just fine.");
                        match self.notify_ready.take() {
                            Some(promise) => promise.fulfill(()).unwrap_or_else(|e| {
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
    }
}

/// Helper for forwarding [MsgEnvelope]s to actor references
impl futures::Sink for ActorRefStrong<DispatchEnvelope> {
    type SinkError = ();
    type SinkItem = DispatchEnvelope;

    fn start_send(&mut self, item: Self::SinkItem) -> StartSend<Self::SinkItem, Self::SinkError> {
        self.tell(item);
        Ok(AsyncSink::Ready)
    }

    fn poll_complete(&mut self) -> Poll<(), Self::SinkError> {
        Ok(Async::Ready(()))
    }

    fn close(&mut self) -> Poll<(), Self::SinkError> {
        Ok(Async::Ready(()))
    }
}

/// Helper for forwarding [MsgEnvelope]s to actor references
impl futures::Sink for DynActorRef {
    type SinkError = ();
    type SinkItem = NetMessage;

    fn start_send(&mut self, item: Self::SinkItem) -> StartSend<Self::SinkItem, Self::SinkError> {
        DynActorRef::enqueue(self, item);
        Ok(AsyncSink::Ready)
    }

    fn poll_complete(&mut self) -> Poll<(), Self::SinkError> {
        Ok(Async::Ready(()))
    }

    fn close(&mut self) -> Poll<(), Self::SinkError> {
        Ok(Async::Ready(()))
    }
}

#[cfg(test)]
mod dispatch_tests {
    use super::{super::*, *};

    use crate::{
        actors::{ActorPath, UniquePath},
        component::{ComponentContext, Provide},
        default_components::DeadletterBox,
        lifecycle::{ControlEvent, ControlPort},
        runtime::{KompactConfig, KompactSystem},
    };
    use bytes::{Buf, BufMut};
    use std::{thread, time::Duration};
    use crate::prelude::Any;

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
        let system = KompactSystem::new(cfg).expect("KompactSystem");
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
            // shouldn't be able to bind on port 80 without root rights
            let net_config =
                NetworkConfig::new("127.0.0.1:0".parse().expect("Address should work"));
            net_config.build()
        });
        println!("Starting KompactSystem");
        let system = KompactSystem::new(cfg).expect("KompactSystem");
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
            .expect("KompicsSystem failed to shut down!");
        println!("System shut down.");
        let mut cfg2 = KompactConfig::new();
        println!("Configuring network");
        cfg2.system_components(DeadletterBox::new, {
            // shouldn't be able to bind on port 80 without root rights
            let net_config =
                NetworkConfig::new(SocketAddr::new("127.0.0.1".parse().unwrap(), port));
            net_config.build()
        });
        println!("Starting 2nd KompactSystem");
        let system2 = KompactSystem::new(cfg2).expect("KompactSystem");
        println!("2nd KompactSystem started just fine.");
        let named_path2 = ActorPath::Named(NamedPath::with_system(
            system2.system_path(),
            vec!["test".into()],
        ));
        println!("Got path: {}", named_path);
        assert_eq!(named_path, named_path2);
        system2
            .shutdown()
            .expect("2nd KompicsSystem failed to shut down!");
    }

    #[test]
    fn test_system_path_timing() {
        let mut cfg = KompactConfig::new();
        println!("Configuring network");
        cfg.system_components(DeadletterBox::new, NetworkConfig::default().build());
        println!("Starting KompactSystem");
        let system = KompactSystem::new(cfg).expect("KompactSystem");
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
        let system = KompactSystem::new(cfg).expect("KompactSystem");
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
    fn remote_delivery_to_registered_actors() {
        let (system, remote) = {
            let system = || {
                let mut cfg = KompactConfig::new();
                cfg.system_components(DeadletterBox::new, NetworkConfig::default().build());
                KompactSystem::new(cfg).expect("KompactSystem")
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
            .expect("Kompics didn't shut down properly");
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
    const PING_PONG_SER: PingPongSer = PingPongSer {};
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
        fn ser_id(&self) -> u64 {
            42
        }

        fn size_hint(&self) -> Option<usize> {
            Some(9)
        }

        fn serialise(&self, buf: &mut BufMut) -> Result<(), SerError> {
            buf.put_i8(PING_ID);
            buf.put_u64(self.i);
            Result::Ok(())
        }

        fn local(self: Box<Self>) -> Result<Box<Any + Send>, Box<Serialisable>> {
            Ok(self)
        }
    }

    impl Serialisable for PongMsg {
        fn ser_id(&self) -> u64 {
            42
        }

        fn size_hint(&self) -> Option<usize> {
            Some(9)
        }

        fn serialise(&self, buf: &mut BufMut) -> Result<(), SerError> {
            buf.put_i8(PONG_ID);
            buf.put_u64(self.i);
            Result::Ok(())
        }

        fn local(self: Box<Self>) -> Result<Box<Any + Send>, Box<Serialisable>> {
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
            //println!("deserializing buf with remaining {} bytes", buf.remaining());
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
    }

    impl PingerAct {
        fn new(target: ActorPath) -> PingerAct {
            PingerAct {
                ctx: ComponentContext::new(),
                target,
                count: 0,
            }
        }
    }

    impl Provide<ControlPort> for PingerAct {
        fn handle(&mut self, event: ControlEvent) -> () {
            match event {
                ControlEvent::Start => {
                    info!(self.ctx.log(), "Starting");
                    self.ctx_mut().initialize_pool();
                    let target = self.target.clone();
                    target.tell_pooled(PingMsg { i: 0 }, self);
                }
                _ => (),
            }
        }
    }

    impl Actor for PingerAct {
        type Message = Never;

        fn receive_local(&mut self, _pong: Self::Message) -> () {
            unimplemented!();
        }

        fn receive_network(&mut self, msg: NetMessage) -> () {
            match msg.try_deserialise::<PongMsg, PingPongSer>() {
                Ok(pong) => {
                    info!(self.ctx.log(), "Got msg {:?}", pong);
                    self.count += 1;
                    if self.count < PING_COUNT {
                        let target = self.target.clone();
                        target.tell_pooled((PingMsg { i: pong.i + 1 }), self);
                    }
                }
                Err(e) => error!(self.ctx.log(), "Error deserialising PongMsg: {:?}", e),
            }
        }
    }

    #[derive(ComponentDefinition)]
    struct PongerAct {
        ctx: ComponentContext<PongerAct>,
    }

    impl PongerAct {
        fn new() -> PongerAct {
            PongerAct {
                ctx: ComponentContext::new(),
            }
        }
    }

    impl Provide<ControlPort> for PongerAct {
        fn handle(&mut self, event: ControlEvent) -> () {
            match event {
                ControlEvent::Start => {
                    info!(self.ctx.log(), "Starting");
                    self.ctx_mut().initialize_pool();
                }
                _ => (),
            }
        }
    }

    impl Actor for PongerAct {
        type Message = Never;

        fn receive_local(&mut self, _ping: Self::Message) -> () {
            unimplemented!();
        }

        fn receive_network(&mut self, msg: NetMessage) -> () {
            let sender = msg.sender().clone();
            match_deser! {msg; {
                ping: PingMsg [PingPongSer] => {
                    info!(self.ctx.log(), "Got msg {:?}", ping);
                    let pong = PongMsg { i: ping.i };
                    sender
                        .tell_pooled(pong, self)
                        .expect("PongMsg should serialise");
                },
                !Err(e) => error!(self.ctx.log(), "Error deserialising PingMsg: {:?}", e),
            }}
        }
    }
}

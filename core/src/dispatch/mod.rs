use super::ComponentDefinition;

use actors::Actor;
use actors::ActorPath;
use actors::ActorRef;
use actors::Dispatcher;
use actors::SystemPath;
use actors::Transport;
use bytes::Buf;
use component::Component;
use component::ComponentContext;
use component::ExecuteResult;
use component::Provide;
use lifecycle::ControlEvent;
use lifecycle::ControlPort;
use std::any::Any;
use std::net::SocketAddr;
use std::sync::Arc;

use actors::NamedPath;
use actors::UniquePath;
use crossbeam::sync::ArcCell;
use dispatch::lookup::ActorStore;
use dispatch::queue_manager::QueueManager;
use futures::Async;
use futures::AsyncSink;
use futures::{self, Poll, StartSend};
use messaging::PathResolvable;
use messaging::RegistrationError;
use messaging::{DispatchEnvelope, EventEnvelope, MsgEnvelope, RegistrationEnvelope};
use net;
use net::events::NetworkEvent;
use net::ConnectionState;
use serialisation::helpers::serialise_msg;
use serialisation::helpers::serialise_to_recv_envelope;
use serialisation::Serialisable;
use std::collections::HashMap;
use std::time::Duration;

pub(crate) mod lookup;
mod queue_manager;

/// Configuration for network dispatcher
#[derive(Clone)]
pub struct NetworkConfig {
    addr: SocketAddr,
}

/// Network-aware dispatcher for messages to remote actors.
#[derive(ComponentDefinition)]
pub struct NetworkDispatcher {
    ctx: ComponentContext<NetworkDispatcher>,
    /// Local map of connection statuses
    connections: HashMap<SocketAddr, ConnectionState>,
    /// Network configuration for this dispatcher
    cfg: NetworkConfig,
    /// Shared lookup structure for mapping [ActorPath]s and [ActorRefs]
    lookup: Arc<ArcCell<ActorStore>>,
    // Fields initialized at [ControlEvent::Start]; they require ComponentContextual awareness
    /// Bridge into asynchronous networking layer
    net_bridge: Option<net::Bridge>,
    /// Management for queuing Frames during network unavailability (conn. init. and MPSC unreadiness)
    queue_manager: Option<QueueManager>,
    /// Reaper which cleans up deregistered actor references in the actor lookup table
    reaper: lookup::gc::ActorRefReaper,
}

// impl NetworkConfig
impl NetworkConfig {
    pub fn new(addr: SocketAddr) -> Self {
        NetworkConfig { addr }
    }
}
impl Default for NetworkConfig {
    fn default() -> Self {
        NetworkConfig {
            addr: "127.0.0.1:8080".parse().unwrap(), // TODO remove hard-coded path
        }
    }
}

// impl NetworkDispatcher
impl NetworkDispatcher {
    pub fn new() -> Self {
        NetworkDispatcher::default()
    }

    pub fn with_config(cfg: NetworkConfig) -> Self {
        let lookup = Arc::new(ArcCell::new(Arc::new(ActorStore::new())));
        let reaper = lookup::gc::ActorRefReaper::default();

        NetworkDispatcher {
            ctx: ComponentContext::new(),
            connections: HashMap::new(),
            cfg,
            lookup,
            net_bridge: None,
            queue_manager: None,
            reaper,
        }
    }

    fn start(&mut self) {
        debug!(self.ctx.log(), "Starting self and network bridge");
        let dispatcher = {
            use actors::ActorRefFactory;
            self.actor_ref()
        };

        let bridge_logger = self.ctx().log().new(o!("owner" => "Bridge"));
        let (mut bridge, events) = net::Bridge::new(self.lookup.clone(), bridge_logger);
        bridge.set_dispatcher(dispatcher.clone());
        bridge.start(self.cfg.addr.clone());

        if let Some(ref ex) = bridge.executor.as_ref() {
            use futures::{Future, Stream};
            ex.spawn(
                events
                    .map(|ev| {
                        MsgEnvelope::Dispatch(DispatchEnvelope::Event(EventEnvelope::Network(ev)))
                    })
                    .forward(dispatcher)
                    .then(|_| Ok(())),
            );
        } else {
            error!(
                self.ctx.log(),
                "No executor found in network bridge; network events can not be handled"
            );
        }
        let queue_manager = QueueManager::new();
        self.net_bridge = Some(bridge);
        self.queue_manager = Some(queue_manager);
    }

    fn schedule_reaper(&mut self) {
        use timer_manager::Timer;

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
            "Scheduling reaping at {:?}ms",
            next_wakeup
        );

        self.schedule_once(Duration::from_millis(next_wakeup), move |target, _id| {
            target.schedule_reaper()
        });
    }

    fn on_event(&mut self, ev: EventEnvelope) {
        match ev {
            EventEnvelope::Network(ev) => match ev {
                NetworkEvent::Connection(addr, conn_state) => self.on_conn_state(addr, conn_state),
                NetworkEvent::Data(_) => {
                    // TODO shouldn't be receiving these here, as they should be routed directly to the ActorRef
                    debug!(self.ctx().log(), "Received important data!");
                }
            },
        }
    }

    fn on_conn_state(&mut self, addr: SocketAddr, mut state: ConnectionState) {
        use self::ConnectionState::*;

        match state {
            Connected(ref mut frame_sender) => {
                debug!(
                    self.ctx().log(),
                    "registering newly connected conn at {:?}",
                    addr
                );

                if let Some(ref mut qm) = self.queue_manager {
                    if qm.has_frame(&addr) {
                        // Drain as much as possible
                        while let Some(frame) = qm.pop_frame(&addr) {
                            if let Err(err) = frame_sender.unbounded_send(frame) {
                                // TODO the underlying channel has been dropped,
                                // indicating that the entire connection is, in fact, not Connected
                                qm.enqueue_frame(err.into_inner(), addr.clone());
                                break;
                            }
                        }
                    }
                }
            }
            Closed => {
                warn!(self.ctx().log(), "connection closed for {:?}", addr);
            }
            Error(_) => {
                error!(self.ctx().log(), "connection error for {:?}", addr);
            }
            ref _other => (), // Don't care
        }
        self.connections.insert(addr, state);
    }

    /// Forwards `msg` up to a local `dst` actor, if it exists.
    ///
    /// # Errors
    /// TODO handle unknown destination actor
    fn route_local(&mut self, src: ActorPath, dst: ActorPath, msg: Box<Serialisable>) {
        use dispatch::lookup::ActorLookup;
        let lookup = self.lookup.get();
        let actor = lookup.get_by_actor_path(&dst);
        if let Some(ref actor) = actor {
            //  TODO err handling
            match msg.local() {
                Ok(boxed_value) => {
                    let src_actor_opt = lookup.get_by_actor_path(&src);
                    if let Some(src_actor) = src_actor_opt {
                        actor.tell_any(boxed_value, src_actor);
                    } else {
                        panic!("Non-local ActorPath ended up in local dispatcher!");
                    }
                }
                Err(msg) => {
                    // local not implemented
                    let envelope = serialise_to_recv_envelope(src, dst, msg).unwrap();
                    actor.enqueue(envelope);
                }
            }
        } else {
            // TODO handle non-existent routes
            error!(self.ctx.log(), "ERR no local actor found at {:?}", dst);
        }
    }

    /// Routes the provided message to the destination, or queues the message until the connection
    /// is available.
    fn route_remote(&mut self, src: ActorPath, dst: ActorPath, msg: Box<Serialisable>) {
        use actors::SystemField;
        use spnl::frames::*;

        let addr = SocketAddr::new(dst.address().clone(), dst.port());
        let frame = {
            let payload = serialise_msg(&src, &dst, msg).expect("s11n error");
            Frame::Data(Data::new(0.into(), 0, payload))
        };

        let state: &mut ConnectionState =
            self.connections.entry(addr).or_insert(ConnectionState::New);
        let next: Option<ConnectionState> = match *state {
            ConnectionState::New | ConnectionState::Closed => {
                debug!(
                    self.ctx.log(),
                    "No connection found; establishing and queuing frame"
                );
                self.queue_manager
                    .as_mut()
                    .map(|ref mut q| q.enqueue_frame(frame, addr));

                if let Some(ref mut bridge) = self.net_bridge {
                    debug!(self.ctx.log(), "Establishing new connection to {:?}", addr);
                    bridge.connect(Transport::TCP, addr).unwrap();
                    Some(ConnectionState::Initializing)
                } else {
                    error!(self.ctx.log(), "No network bridge found; dropping message");
                    Some(ConnectionState::Closed)
                }
            }
            ConnectionState::Connected(ref mut tx) => {
                if let Some(ref mut qm) = self.queue_manager {
                    if qm.has_frame(&addr) {
                        qm.enqueue_frame(frame, addr.clone());
                        qm.try_drain(addr, tx)
                    } else {
                        // Send frame
                        if let Err(err) = tx.unbounded_send(frame) {
                            // Unbounded senders report errors only if dropped
                            let mut next = Some(ConnectionState::Closed);
                            // Consume error and retrieve failed Frame
                            let frame = err.into_inner();
                            qm.enqueue_frame(frame, addr);
                            next
                        } else {
                            None
                        }
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
                    .map(|ref mut q| q.enqueue_frame(frame, addr));
                None
            }
            _ => None,
        };

        if let Some(next) = next {
            *state = next;
        }
    }

    /// Forwards `msg` to destination described by `dst`, routing it across the network
    /// if needed.
    fn route(&mut self, src: PathResolvable, dst_path: ActorPath, msg: Box<Serialisable>) {
        let src_path = match src {
            PathResolvable::Path(actor_path) => actor_path.clone(),
            PathResolvable::Alias(alias) => {
                ActorPath::Named(NamedPath::with_system(self.system_path(), vec![alias]))
            }
            PathResolvable::ActorId(uuid) => {
                ActorPath::Unique(UniquePath::with_system(self.system_path(), uuid.clone()))
            }
            PathResolvable::System => self.actor_path(),
        };

        let proto = {
            use actors::SystemField;
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

impl Default for NetworkDispatcher {
    fn default() -> Self {
        NetworkDispatcher::with_config(NetworkConfig::default())
    }
}

impl Actor for NetworkDispatcher {
    fn receive_local(&mut self, sender: ActorRef, msg: Box<Any>) {
        debug!(self.ctx.log(), "Received LOCAL {:?} from {:?}", msg, sender);
    }
    fn receive_message(&mut self, sender: ActorPath, ser_id: u64, _buf: &mut Buf) {
        debug!(
            self.ctx.log(),
            "Received buffer with id {:?} from {:?}",
            ser_id,
            sender
        );
    }
}

impl Dispatcher for NetworkDispatcher {
    fn receive(&mut self, env: DispatchEnvelope) {
        match env {
            DispatchEnvelope::Cast(_) => {
                // Should not be here!
                error!(self.ctx.log(), "Received a cast envelope");
            }
            DispatchEnvelope::Msg { src, dst, msg } => {
                // Look up destination (local or remote), then route or err
                self.route(src, dst, msg);
            }
            DispatchEnvelope::Registration(reg) => {
                use dispatch::lookup::ActorLookup;

                match reg {
                    RegistrationEnvelope::Register(actor, path, mut promise) => {
                        let prev = self.lookup.get();
                        let mut next = (*prev).clone();
                        if next.contains(&path) {
                            warn!(self.ctx.log(), "Detected duplicate path during registration. The path will not be re-registered");

                            if let Some(promise) = promise.take() {
                                promise.fulfill(Err(RegistrationError::DuplicateEntry));
                            }
                            // Return early; prevent registration from taking place
                            return;
                        }
                        next.insert(actor, path);
                        self.lookup.set(Arc::new(next));

                        if let Some(promise) = promise.take() {
                            promise.fulfill(Ok(()));
                        }

                        if !self.reaper.is_scheduled() {
                            self.schedule_reaper();
                        }
                    }
                }
            }
            DispatchEnvelope::Event(ev) => self.on_event(ev),
        }
    }

    /// Generates a [SystemPath](kompics::actors) from this dispatcher's configuration
    fn system_path(&mut self) -> SystemPath {
        // TODO get protocol from configuration
        SystemPath::new(Transport::TCP, self.cfg.addr.ip(), self.cfg.addr.port())
    }
}

impl Provide<ControlPort> for NetworkDispatcher {
    fn handle(&mut self, event: ControlEvent) {
        match event {
            ControlEvent::Start => {
                self.start();
            }
            ControlEvent::Stop => info!(self.ctx.log(), "Stopping"),
            ControlEvent::Kill => info!(self.ctx.log(), "Killed"),
        }
    }
}

/// Helper for forwarding [MsgEnvelope]s to actor references
impl futures::Sink for ActorRef {
    type SinkItem = MsgEnvelope;
    type SinkError = ();

    fn start_send(&mut self, item: Self::SinkItem) -> StartSend<Self::SinkItem, Self::SinkError> {
        ActorRef::enqueue(self, item);
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
    use super::super::*;
    use super::*;

    use actors::ActorPath;
    use actors::UniquePath;
    use bytes::{Buf, BufMut};
    use component::ComponentContext;
    use component::Provide;
    use default_components::DeadletterBox;
    use lifecycle::ControlEvent;
    use lifecycle::ControlPort;
    use runtime::KompicsConfig;
    use runtime::KompicsSystem;
    use std::sync::atomic::{AtomicUsize, Ordering, ATOMIC_USIZE_INIT};
    use std::thread;
    use std::time::Duration;

    // Thread-safe TCP port incrementer to enable parallel network tests
    static GLOBAL_PORT_INCR: AtomicUsize = ATOMIC_USIZE_INIT;
    const BASE_PORT: u16 = 8080;

    fn tcp_listening_port() -> u16 {
        let count = GLOBAL_PORT_INCR.fetch_add(1, Ordering::SeqCst) + 1;
        BASE_PORT + (count as u16)
    }

    #[test]
    fn named_registration() {
        const ACTOR_NAME: &str = "ponger";

        let mut cfg = KompicsConfig::new();
        cfg.system_components(DeadletterBox::new, || {
            let net_config = NetworkConfig {
                addr: SocketAddr::new("127.0.0.1".parse().unwrap(), tcp_listening_port()),
            };
            NetworkDispatcher::with_config(net_config)
        });
        let system = KompicsSystem::new(cfg);
        let ponger = system.create(PongerAct::new);
        system.start(&ponger);

        let res = system
            .register_by_alias(&ponger, ACTOR_NAME)
            .await_timeout(Duration::from_millis(1000))
            .expect("Registration never completed.");
        assert!(
            res.is_ok(),
            "Single registration with unique alias should succeed."
        );

        let res = system
            .register_by_alias(&ponger, ACTOR_NAME)
            .await_timeout(Duration::from_millis(1000))
            .expect("Registration never completed.");

        assert_eq!(
            res,
            Err(RegistrationError::DuplicateEntry),
            "Duplicate alias registration should fail."
        );

        system
            .kill_notify(ponger)
            .await_timeout(Duration::from_millis(1000))
            .expect("Ponger did not die");
        thread::sleep(Duration::from_millis(1000));

        system
            .shutdown()
            .expect("Kompics didn't shut down properly");
    }

    #[test]
    /// Sets up a single KompicsSystem with 2x Pingers and Pongers. One Ponger is registered by UUID,
    /// the other by a custom name. One Pinger communicates with the UUID-registered Ponger,
    /// the other with the named Ponger. Both sets are expected to exchange PING_COUNT ping-pong
    /// messages.
    fn remote_delivery_to_registered_actors() {
        let (system, remote) = {
            let system = || {
                let mut cfg = KompicsConfig::new();
                cfg.system_components(DeadletterBox::new, || {
                    let net_config = NetworkConfig {
                        addr: SocketAddr::new("127.0.0.1".parse().unwrap(), tcp_listening_port()),
                    };
                    NetworkDispatcher::with_config(net_config)
                });
                KompicsSystem::new(cfg)
            };
            (system(), system())
        };
        let ponger_unique = remote.create_and_register(PongerAct::new);
        let ponger_named = remote.create_and_register(PongerAct::new);
        remote.register_by_alias(&ponger_named, "custom_name");

        let named_path = ActorPath::Named(NamedPath::with_system(
            remote.system_path(),
            vec!["custom_name".into()],
        ));

        let unique_path = ActorPath::Unique(UniquePath::with_system(
            remote.system_path(),
            ponger_unique.id().clone(),
        ));

        let pinger_unique = system.create_and_register(move || PingerAct::new(unique_path));
        let pinger_named = system.create_and_register(move || PingerAct::new(named_path));

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
            .await_timeout(Duration::from_millis(1000))
            .expect("Pinger never stopped!");
        pongfu
            .await_timeout(Duration::from_millis(1000))
            .expect("Ponger never died!");
        pingfn
            .await_timeout(Duration::from_millis(1000))
            .expect("Pinger never stopped!");
        pongfn
            .await_timeout(Duration::from_millis(1000))
            .expect("Ponger never died!");
        pinger_named.on_definition(|c| {
            assert_eq!(c.remote_count, PING_COUNT);
            assert_eq!(c.local_count, 0);
        });
        pinger_unique.on_definition(|c| {
            assert_eq!(c.remote_count, PING_COUNT);
            assert_eq!(c.local_count, 0);
        });

        system
            .shutdown()
            .expect("Kompics didn't shut down properly");
    }

    const PING_COUNT: u64 = 10;

    #[test]
    fn local_delivery() {
        let mut cfg = KompicsConfig::new();
        cfg.system_components(DeadletterBox::new, NetworkDispatcher::default);
        let system = KompicsSystem::new(cfg);

        let ponger = system.create_and_register(PongerAct::new);
        // Construct ActorPath with system's `proto` field explicitly set to LOCAL
        let unique_path = UniquePath::new(
            Transport::LOCAL,
            "127.0.0.1".parse().expect("hardcoded IP"),
            8080,
            ponger.id().clone(),
        );
        let ponger_path = ActorPath::Unique(unique_path);
        let pinger = system.create_and_register(move || PingerAct::new(ponger_path));

        system.start(&ponger);
        system.start(&pinger);

        thread::sleep(Duration::from_millis(1000));

        let pingf = system.stop_notify(&pinger);
        let pongf = system.kill_notify(ponger);
        pingf
            .await_timeout(Duration::from_millis(1000))
            .expect("Pinger never stopped!");
        pongf
            .await_timeout(Duration::from_millis(1000))
            .expect("Ponger never died!");
        pinger.on_definition(|c| {
            assert_eq!(c.local_count, PING_COUNT);
            assert_eq!(c.remote_count, 0);
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
    const PING_PONG_SER: PingPongSer = PingPongSer {};
    const PING_ID: i8 = 1;
    const PONG_ID: i8 = 2;
    impl Serialiser<PingMsg> for PingPongSer {
        fn serid(&self) -> u64 {
            42 // because why not^^
        }
        fn size_hint(&self) -> Option<usize> {
            Some(9)
        }
        fn serialise(&self, v: &PingMsg, buf: &mut BufMut) -> Result<(), SerError> {
            buf.put_i8(PING_ID);
            buf.put_u64_be(v.i);
            Result::Ok(())
        }
    }

    impl Serialiser<PongMsg> for PingPongSer {
        fn serid(&self) -> u64 {
            42 // because why not^^
        }
        fn size_hint(&self) -> Option<usize> {
            Some(9)
        }
        fn serialise(&self, v: &PongMsg, buf: &mut BufMut) -> Result<(), SerError> {
            buf.put_i8(PONG_ID);
            buf.put_u64_be(v.i);
            Result::Ok(())
        }
    }
    impl Deserialiser<PingMsg> for PingPongSer {
        fn deserialise(buf: &mut Buf) -> Result<PingMsg, SerError> {
            if buf.remaining() < 9 {
                return Err(SerError::InvalidData(format!(
                    "Serialised typed has 9bytes but only {}bytes remain in buffer.",
                    buf.remaining()
                )));
            }
            match buf.get_i8() {
                PING_ID => {
                    let i = buf.get_u64_be();
                    Ok(PingMsg { i })
                }
                PONG_ID => Err(SerError::InvalidType(
                    "Found PongMsg, but expected PingMsg.".into(),
                )),
                _ => Err(SerError::InvalidType(
                    "Found unkown id, but expected PingMsg.".into(),
                )),
            }
        }
    }
    impl Deserialiser<PongMsg> for PingPongSer {
        fn deserialise(buf: &mut Buf) -> Result<PongMsg, SerError> {
            if buf.remaining() < 9 {
                return Err(SerError::InvalidData(format!(
                    "Serialised typed has 9bytes but only {}bytes remain in buffer.",
                    buf.remaining()
                )));
            }
            match buf.get_i8() {
                PONG_ID => {
                    let i = buf.get_u64_be();
                    Ok(PongMsg { i })
                }
                PING_ID => Err(SerError::InvalidType(
                    "Found PingMsg, but expected PongMsg.".into(),
                )),
                _ => Err(SerError::InvalidType(
                    "Found unkown id, but expected PongMsg.".into(),
                )),
            }
        }
    }

    #[derive(ComponentDefinition)]
    struct PingerAct {
        ctx: ComponentContext<PingerAct>,
        target: ActorPath,
        local_count: u64,
        remote_count: u64,
    }

    impl PingerAct {
        fn new(target: ActorPath) -> PingerAct {
            PingerAct {
                ctx: ComponentContext::new(),
                target,
                local_count: 0,
                remote_count: 0,
            }
        }

        fn total_count(&self) -> u64 {
            self.local_count + self.remote_count
        }
    }

    impl Provide<ControlPort> for PingerAct {
        fn handle(&mut self, event: ControlEvent) -> () {
            match event {
                ControlEvent::Start => {
                    info!(self.ctx.log(), "Starting");
                    self.target.tell((PingMsg { i: 0 }, PING_PONG_SER), self);
                }
                _ => (),
            }
        }
    }

    impl Actor for PingerAct {
        fn receive_local(&mut self, sender: ActorRef, msg: Box<Any>) -> () {
            match msg.downcast_ref::<PongMsg>() {
                Some(ref pong) => {
                    info!(self.ctx.log(), "Got local Pong({})", pong.i);
                    self.local_count += 1;
                    if self.total_count() < PING_COUNT {
                        self.target
                            .tell((PingMsg { i: pong.i + 1 }, PING_PONG_SER), self);
                    }
                }
                None => error!(self.ctx.log(), "Got unexpected local msg from {}.", sender),
            }
        }
        fn receive_message(&mut self, sender: ActorPath, ser_id: u64, buf: &mut Buf) -> () {
            if ser_id == Serialiser::<PongMsg>::serid(&PING_PONG_SER) {
                let r: Result<PongMsg, SerError> = PingPongSer::deserialise(buf);
                match r {
                    Ok(pong) => {
                        info!(self.ctx.log(), "Got msg Pong({})", pong.i);
                        self.remote_count += 1;
                        if self.total_count() < PING_COUNT {
                            self.target
                                .tell((PingMsg { i: pong.i + 1 }, PING_PONG_SER), self);
                        }
                    }
                    Err(e) => error!(self.ctx.log(), "Error deserialising PongMsg: {:?}", e),
                }
            } else {
                error!(
                    self.ctx.log(),
                    "Got message with unexpected serialiser {} from {}",
                    ser_id,
                    sender
                );
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
                }
                _ => (),
            }
        }
    }

    impl Actor for PongerAct {
        fn receive_local(&mut self, sender: ActorRef, msg: Box<Any>) -> () {
            match msg.downcast_ref::<PingMsg>() {
                Some(ref ping) => {
                    info!(self.ctx.log(), "Got local Ping({})", ping.i);
                    sender.tell(Box::new(PongMsg { i: ping.i }), self);
                }
                None => error!(self.ctx.log(), "Got unexpected local msg from {}.", sender),
            }
        }
        fn receive_message(&mut self, sender: ActorPath, ser_id: u64, buf: &mut Buf) -> () {
            if ser_id == Serialiser::<PingMsg>::serid(&PING_PONG_SER) {
                let r: Result<PingMsg, SerError> = PingPongSer::deserialise(buf);
                match r {
                    Ok(ping) => {
                        info!(self.ctx.log(), "Got msg Ping({})", ping.i);
                        sender.tell((PongMsg { i: ping.i }, PING_PONG_SER), self);
                    }
                    Err(e) => error!(self.ctx.log(), "Error deserialising PingMsg: {:?}", e),
                }
            } else {
                error!(
                    self.ctx.log(),
                    "Got message with unexpected serialiser {} from {}",
                    ser_id,
                    sender
                );
            }
        }
    }
}

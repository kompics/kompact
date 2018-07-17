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

use actors::UniquePath;
use dispatch::lookup::ActorStore;
use dispatch::queue_manager::QueueManager;
use futures::sync;
use futures::sync::mpsc::TrySendError;
use futures::Async;
use futures::AsyncSink;
use futures::{self, Poll, StartSend};
use messaging::DispatchEnvelope;
use messaging::EventEnvelope;
use messaging::MsgEnvelope;
use messaging::PathResolvable;
use messaging::RegistrationEnvelope;
use net;
use net::events::NetworkEvent;
use net::ConnectionState;
use serialisation::helpers::serialise_to_recv_envelope;
use serialisation::Serialisable;
use spnl;
use spnl::frames::Frame;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::Mutex;
use KompicsLogger;
use crossbeam::sync::ArcCell;

pub(crate) mod lookup;
mod queue_manager;

/// Configuration for network dispatcher
pub struct NetworkConfig {
    addr: SocketAddr,
}

struct DispatchCounts {
    seq_num: u64,
}

/// Network-aware dispatcher for messages to remote actors.
#[derive(ComponentDefinition)]
pub struct NetworkDispatcher {
    ctx: ComponentContext<NetworkDispatcher>,
    /// Stateful information
    counts: DispatchCounts,
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
}

// impl NetworkConfig
impl Default for NetworkConfig {
    fn default() -> Self {
        NetworkConfig {
            addr: "127.0.0.1:8080".parse().unwrap(), // TODO remove hard-coded path
        }
    }
}

// impl DispatchCounts
impl DispatchCounts {
    fn new() -> Self {
        DispatchCounts { seq_num: 0 }
    }

    fn incr_seq_num(&mut self) -> u64 {
        let seq_num = self.seq_num;
        self.seq_num += 1;
        seq_num
    }
}

// impl NetworkDispatcher
impl NetworkDispatcher {
    pub fn new() -> Self {
        NetworkDispatcher::default()
    }

    pub fn with_config(cfg: NetworkConfig) -> Self {
        NetworkDispatcher {
            ctx: ComponentContext::new(),
            counts: DispatchCounts::new(),
            connections: HashMap::new(),
            cfg,
            lookup: Arc::new(ArcCell::new(Arc::new(ActorStore::new()))),
            net_bridge: None,
            queue_manager: None,
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
        let queue_manager = QueueManager::new(self.ctx().log().new(o!("owner" => "QueueManager")));
        self.net_bridge = Some(bridge);
        self.queue_manager = Some(queue_manager);
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
                            if let Err(err) = frame_sender.try_send(frame) {
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
    /// FIXME this fn
    fn route_local(&mut self, src: PathResolvable, dst: ActorPath, msg: Box<Serialisable>) {
        //        let actor = match dst {
        //            ActorPath::Unique(ref up) => self.lookup.get_by_uuid(up.uuid_ref()),
        //            ActorPath::Named(ref np) => self.lookup.get_by_named_path(&np.path_ref()),
        //        };
        //
        //        if let Some(actor) = actor {
        //            //  TODO err handling
        //            match msg.local() {
        //                Ok(boxed_value) => {
        //                    let src_actor_opt = match src {
        //                        ActorPath::Unique(ref up) => self.lookup.get_by_uuid(up.uuid_ref()),
        //                        ActorPath::Named(ref np) => self.lookup.get_by_named_path(&np.path_ref()),
        //                    };
        //                    if let Some(src_actor) = src_actor_opt {
        //                        actor.tell_any(boxed_value, src_actor);
        //                    } else {
        //                        panic!("Non-local ActorPath ended up in local dispatcher!");
        //                    }
        //                }
        //                Err(msg) => {
        //                    // local not implemented
        //                    let envelope = serialise_to_recv_envelope(src, dst, msg).unwrap();
        //                    actor.enqueue(envelope);
        //                }
        //            }
        //        } else {
        //            // TODO handle non-existent routes
        //            error!(self.ctx.log(), "ERR no local actor found at {:?}", dst);
        //        }
    }

    /// Routes the provided message to the destination, or queues the message until the connection
    /// is available.
    fn route_remote(&mut self, src: PathResolvable, dst: ActorPath, msg: Box<Serialisable>) {
        use actors::SystemField;
        use spnl::frames::*;

        debug!(self.ctx.log(), "Routing remote message {:?}", msg);

        // TODO serialize entire envelope into frame's payload, figure out deserialisation scheme as well
        // TODO ship over to network/tokio land

        let addr = SocketAddr::new(dst.address().clone(), dst.port());

        let frame = {
            let payload = self.serialize(&src, &dst, msg);
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
                        if let Err(err) = tx.try_send(frame) {
                            let mut next: Option<ConnectionState> = None;
                            if err.is_disconnected() {
                                next = Some(ConnectionState::Closed)
                            }

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
    fn route(&mut self, src: PathResolvable, dst: ActorPath, msg: Box<Serialisable>) {
        let proto = {
            use actors::SystemField;
            let dst_sys = dst.system();
            SystemField::protocol(dst_sys)
        };
        match proto {
            Transport::LOCAL => {
                self.route_local(src, dst, msg);
            }
            Transport::TCP => {
                self.route_remote(src, dst, msg);
            }
            Transport::UDP => {
                error!(self.ctx.log(), "UDP routing not supported yet");
            }
        }
    }

    fn actor_path(&mut self) -> ActorPath {
        let uuid = self.ctx.id();
        ActorPath::Unique(UniquePath::with_system(self.system_path(), uuid.clone()))
    }

    // TODO refactor this
    fn serialize(
        &mut self,
        src: &PathResolvable,
        dst: &ActorPath,
        msg: Box<Serialisable>,
    ) -> spnl::bytes_ext::Bytes {
        //        use spnl::bytes_ext::{Bytes, BytesMut, Buf, BufMut};
        use bytes::{Buf, BufMut, Bytes, BytesMut};
        let mut size = msg.size_hint().unwrap_or(0);
        let src_path = match src {
            PathResolvable::Path(actor_path) => actor_path.clone(),
            PathResolvable::ActorId(uuid) => {
                ActorPath::Unique(UniquePath::with_system(self.system_path(), uuid.clone()))
            }
            &PathResolvable::System => self.actor_path()
        };
        size += 8; // ser_id: u64
        size += src_path.size_hint().unwrap_or(0);
        size += dst.size_hint().unwrap_or(0);

        let mut buf = BytesMut::with_capacity(size);
        Serialisable::serialise(&src_path, &mut buf).expect("s11n error");
        Serialisable::serialise(dst, &mut buf).expect("s11n error");
        buf.put_u64(msg.serid());
        Serialisable::serialise(&(*msg), &mut buf).expect("s11n error");
        spnl::bytes_ext::Bytes::from(buf.freeze().as_ref())
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
                    RegistrationEnvelope::Register(actor, path) => {
                        let prev = self.lookup.get();
                        let mut next = (*prev).clone();
                        next.insert(actor, path);
                        self.lookup.set(Arc::new(next));
                    }
                    RegistrationEnvelope::Deregister(actor_path) => {
                        debug!(self.ctx.log(), "Deregistering actor at {:?}", actor_path);
                        // TODO handle
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
    use ports::Port;
    use runtime::KompicsConfig;
    use runtime::KompicsSystem;
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;
    use uuid::Uuid;

    #[test]
    fn registration() {
        let mut cfg = KompicsConfig::new();
        cfg.system_components(DeadletterBox::new, NetworkDispatcher::default);
        let system = KompicsSystem::new(cfg);

        let component = system.create_and_register(TestComponent::new);

        // FIXME @Johan

        let sys_path = system.system_path();
        let reference =
            ActorPath::Unique(UniquePath::with_system(sys_path, component.id().clone()));

        reference.tell("Network me, dispatch!", &system);
        thread::sleep(Duration::from_millis(10));

        reference.tell("Network me, dispatch!", &system);
        // Sleep; allow the system to progress
        thread::sleep(Duration::from_secs(10));

        match system.shutdown() {
            Ok(_) => println!("Successful shutdown"),
            Err(_) => eprintln!("Error shutting down system"),
        }
    }

    const PING_COUNT: u64 = 10;

    #[test]
    #[ignore]
    fn local_delivery() {
        let mut cfg = KompicsConfig::new();
        cfg.system_components(DeadletterBox::new, NetworkDispatcher::default);
        let system = KompicsSystem::new(cfg);

        let ponger = system.create_and_register(PongerAct::new);
        // FIXME @Johan
        // let ponger_path = ponger.actor_path();
        // let pinger = system.create_and_register(move || PingerAct::new(ponger_path));

        // system.start(&ponger);
        // system.start(&pinger);

        // thread::sleep(Duration::from_millis(1000));

        // let pingf = system.stop_notify(&pinger);
        // let pongf = system.kill_notify(ponger);
        // pingf
        //     .await_timeout(Duration::from_millis(1000))
        //     .expect("Pinger never stopped!");
        // pongf
        //     .await_timeout(Duration::from_millis(1000))
        //     .expect("Ponger never died!");
        // pinger.on_definition(|c| {
        //     assert_eq!(c.local_count, PING_COUNT);
        // });

        // system
        //     .shutdown()
        //     .expect("Kompics didn't shut down properly");
    }

    #[derive(ComponentDefinition, Actor)]
    struct TestComponent {
        ctx: ComponentContext<TestComponent>,
    }

    impl TestComponent {
        fn new() -> Self {
            TestComponent {
                ctx: ComponentContext::new(),
            }
        }
    }

    impl Provide<ControlPort> for TestComponent {
        fn handle(&mut self, _event: <ControlPort as Port>::Request) -> () {}
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
            buf.put_u64(v.i);
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
            buf.put_u64(v.i);
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
                    let i = buf.get_u64();
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
                    let i = buf.get_u64();
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
        msg_count: u64,
    }

    impl PingerAct {
        fn new(target: ActorPath) -> PingerAct {
            PingerAct {
                ctx: ComponentContext::new(),
                target,
                local_count: 0,
                msg_count: 0,
            }
        }

        fn total_count(&self) -> u64 {
            self.local_count + self.msg_count
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
                        self.msg_count += 1;
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

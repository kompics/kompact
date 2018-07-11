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

mod lookup;

use dispatch::lookup::ActorLookup;
use messaging::DispatchEnvelope;
use messaging::RegistrationEnvelope;
use serialisation::helpers::serialise_to_recv_envelope;
use serialisation::Serialisable;
use std::thread;
/// Configuration for network dispatcher
pub struct NetworkConfig {
    addr: SocketAddr,
}

/// Network-aware dispatcher for messages to remote actors.
#[derive(ComponentDefinition)]
pub struct NetworkDispatcher {
    ctx: ComponentContext<NetworkDispatcher>,
    cfg: NetworkConfig,
    lookup: ActorLookup,
    net_bridge: (),
}

// impl NetworkConfig
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
        let builder = thread::Builder::new();
        let handle = builder.name("tokio-bridge".into()).spawn(|| {
            println!("[TOKIO-THREAD] spawned"); // TODO get a logger here, maybe?
                                                // TODO cross the vast asynchronous barrier here :-\
                                                // maybe using futures::mpsc is the best way here, by having a sender
        });

        NetworkDispatcher {
            ctx: ComponentContext::new(),
            cfg,
            lookup: ActorLookup::new(),
            net_bridge: (),
        }
    }

    /// Forwards `msg` up to a local `dst` actor, if it exists.
    ///
    /// # Errors
    /// TODO handle unknown destination actor
    fn route_local(&self, src: ActorPath, dst: ActorPath, msg: Box<Serialisable>) {
        let actor = match dst {
            ActorPath::Unique(ref up) => self.lookup.get_by_uuid(up.uuid_ref()),
            ActorPath::Named(ref np) => self.lookup.get_by_named_path(&np.path_ref()),
        };

        if let Some(actor) = actor {
            //  TODO err handling
            match msg.local() {
                Ok(boxed_value) => {
                    let src_actor_opt = match src {
                        ActorPath::Unique(ref up) => self.lookup.get_by_uuid(up.uuid_ref()),
                        ActorPath::Named(ref np) => self.lookup.get_by_named_path(&np.path_ref()),
                    };
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
            error!(self.ctx.log(), "ERR on local actor found at {:?}", dst);
        }
    }

    /// Forwards `msg` to remote `dst` actor via the network layer.
    fn route_remote(&self, src: ActorPath, dst: ActorPath, msg: Box<Serialisable>) {
        trace!(self.ctx.log(), "Routing remote message {:?}", msg);
        // TODO seralize entire envelope into frame's payload, figure out deserialisation scheme as well
        // TODO ship over to network/tokio land
    }

    /// Forwards `msg` to destination described by `dst`, routing it across the network
    /// if needed.
    fn route(&self, src: ActorPath, dst: ActorPath, msg: Box<Serialisable>) {
        use actors::{ActorRefFactory, SystemField};

        let proto = {
            let dst_sys = dst.system();
            SystemField::protocol(dst_sys)
        };

        match proto {
            Transport::LOCAL => {
                self.route_local(src, dst, msg);
            }
            Transport::TCP | Transport::UDP => {
                self.route_remote(src, dst, msg);
            }
        }
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
                //self.route(src, dst, msg);
                // FIXME @Johan
            }
            DispatchEnvelope::Registration(reg) => {
                match reg {
                    RegistrationEnvelope::Register(actor) => {
                        self.lookup.insert(actor);
                    }
                    RegistrationEnvelope::Deregister(actor_path) => {
                        debug!(self.ctx.log(), "Deregistering actor at {:?}", actor_path);
                        // TODO handle
                    }
                }
            }
        }
    }

    fn system_path(&mut self) -> SystemPath {
        SystemPath::new(Transport::LOCAL, self.cfg.addr.ip(), self.cfg.addr.port())
    }
}

impl Provide<ControlPort> for NetworkDispatcher {
    fn handle(&mut self, event: ControlEvent) {
        match event {
            ControlEvent::Start => debug!(self.ctx.log(), "Starting"),
            ControlEvent::Stop => info!(self.ctx.log(), "Stopping"),
            ControlEvent::Kill => info!(self.ctx.log(), "Killed"),
        }
    }
}

#[cfg(test)]
mod dispatch_tests {
    use super::super::*;
    use super::*;

    use bytes::{Buf, BufMut};
    use component::ComponentContext;
    use component::Provide;
    use component::Require;
    use default_components::DeadletterBox;
    use lifecycle::ControlEvent;
    use lifecycle::ControlPort;
    use ports::Port;
    use ports::RequiredPort;
    use runtime::KompicsConfig;
    use runtime::KompicsSystem;
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    #[test]
    #[ignore]
    fn registration() {
        let mut cfg = KompicsConfig::new();
        cfg.system_components(DeadletterBox::new, NetworkDispatcher::default);
        let system = KompicsSystem::new(cfg);

        let component = system.create_and_register(TestComponent::new);
        // FIXME @Johan
        // let path = component.actor_path();
        // let port = component.on_definition(|this| this.rec_port.share());

        // system.trigger_i(Arc::new(String::from("local indication")), port);

        // let reference = component.actor_path();

        // reference.tell("Network me, dispatch!", &system);

        // // Sleep; allow the system to progress
        // thread::sleep(Duration::from_millis(1000));

        // match system.shutdown() {
        //     Ok(_) => println!("Successful shutdown"),
        //     Err(_) => eprintln!("Error shutting down system"),
        // }
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

    // struct definitions
    struct TestPort;

    #[derive(ComponentDefinition, Actor)]
    struct TestComponent {
        ctx: ComponentContext<TestComponent>,
        counter: u64,
        rec_port: RequiredPort<TestPort, TestComponent>,
    }

    // impl TestPort
    impl Port for TestPort {
        type Indication = Arc<String>;
        type Request = Arc<u64>;
    }

    // impl TestComponent
    impl TestComponent {
        fn new() -> Self {
            TestComponent {
                ctx: ComponentContext::new(),
                counter: 0,
                rec_port: RequiredPort::new(),
            }
        }
    }

    impl Provide<ControlPort> for TestComponent {
        fn handle(&mut self, event: ControlEvent) -> () {
            match event {
                ControlEvent::Start => {
                    info!(self.ctx.log(), "Starting");
                }
                _ => (),
            }
        }
    }

    impl Require<TestPort> for TestComponent {
        fn handle(&mut self, event: Arc<String>) -> () {
            info!(self.ctx.log(), "Handling {:?}", event);
        }
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

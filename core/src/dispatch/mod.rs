use super::ComponentDefinition;
use super::Require;
use actors::Actor;
use actors::ActorPath;
use actors::ActorRef;
use actors::Dispatcher;
use actors::SystemPath;
use actors::Transport;
use bytes::{Buf, BufMut};
use component::Component;
use component::ComponentContext;
use component::ExecuteResult;
use component::Provide;
use lifecycle::ControlEvent;
use lifecycle::ControlPort;
use messaging::ReceiveEnvelope;
use ports::Port;
use ports::ProvidedPort;
use ports::ProvidedRef;
use ports::RequiredPort;
use std::any::Any;
use std::net::SocketAddr;
use std::sync::Arc;

mod lookup;

use bytes::BytesMut;
use dispatch::lookup::ActorLookup;
use messaging::CastEnvelope;
use messaging::DispatchEnvelope;
use messaging::MsgEnvelope;
use messaging::RegistrationEnvelope;
use serialisation::helpers::serialise_to_recv_envelope;
use serialisation::Serialisable;
use std::thread;
use std::thread::Thread;

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
            println!("[TOKIO-THREAD] spawned");
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
            // TODO bypass s11n and bubble up statically
            let envelope = serialise_to_recv_envelope(src, dst, msg).unwrap();
            actor.enqueue(envelope);
        } else {
            // TODO handle non-existent routes
            println!("[NetworkDispatcher] ERR on local actor found at {:?}", dst);
        }
    }

    /// Forwards `msg` to remote `dst` actor via the network layer.
    fn route_remote(&self, src: ActorPath, dst: ActorPath, msg: Box<Serialisable>) {
        println!("[NetworkDispatcher] routing remote message {:?}", msg);
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
        println!(
            "[NetworkDispatcher] received LOCAL {:?} from {:?}",
            msg, sender
        );
    }
    fn receive_message(&mut self, sender: ActorPath, ser_id: u64, buf: &mut Buf) {
        println!(
            "[NetworkDispatcher] received buffer with id {:?} from {:?}",
            ser_id, sender
        );
    }
}

impl Dispatcher for NetworkDispatcher {
    fn receive(&mut self, env: DispatchEnvelope) {
        match env {
            DispatchEnvelope::Cast(_) => {
                // Should not be here!
                println!("[NetworkDispatcher] err! received a cast envelope");
            }
            DispatchEnvelope::Msg { src, dst, msg } => {
                // Look up destination (local or remote), then route or err
                self.route(src, dst, msg);
            }
            DispatchEnvelope::Registration(reg) => {
                match reg {
                    RegistrationEnvelope::Register(actor) => {
                        self.lookup.insert(actor);
                    }
                    RegistrationEnvelope::Deregister(actor_path) => {
                        println!(
                            "[NetworkDispatcher] deregistering actor at {:?}",
                            actor_path
                        );
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
            ControlEvent::Start => println!("[NetworkDispatcher] starting"),
            ControlEvent::Stop => println!("[NetworkDispatcher] stopping"),
            ControlEvent::Kill => println!("[NetworkDispatcher] killed"),
        }
    }
}

#[cfg(test)]
mod dispatch_tests {
    use super::super::*;
    use super::*;

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
    use std::time;

    #[test]
    fn registration() {
        let mut cfg = KompicsConfig::new();
        cfg.system_components(DeadletterBox::new, || NetworkDispatcher::default());
        let system = KompicsSystem::new(cfg);

        let component = system.create_and_register(TestComponent::new);
        let path = component.actor_path();
        let port = component.on_definition(|this| this.rec_port.share());

        system.trigger_i(Arc::new(String::from("local indication")), port);

        let reference = component.actor_path();

        reference.tell("Network me, dispatch!", &system);

        match system.shutdown() {
            Ok(_) => println!("Successful shutdown"),
            Err(_) => println!("Error shutting down system"),
        }
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
                    println!("[TestComponent] starting");
                }
                _ => (),
            }
        }
    }

    impl Require<TestPort> for TestComponent {
        fn handle(&mut self, event: Arc<String>) -> () {
            println!("[TestComponent] handling {:?}", event);
        }
    }
}

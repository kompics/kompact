#![allow(clippy::unused_unit)]
use kompact::{prelude::*, serde_serialisers::*};
use kompact_examples::trusting::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::Duration,
};

// ANCHOR: messages
#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
struct CheckIn;
impl SerialisationId for CheckIn {
    const SER_ID: SerId = 2345;
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct UpdateProcesses(Vec<ActorPath>);
impl SerialisationId for UpdateProcesses {
    const SER_ID: SerId = 3456;
}
// ANCHOR_END: messages

// ANCHOR: state
#[derive(ComponentDefinition)]
struct BootstrapServer {
    ctx: ComponentContext<Self>,
    processes: HashSet<ActorPath>,
}
impl BootstrapServer {
    fn new() -> Self {
        BootstrapServer {
            ctx: ComponentContext::uninitialised(),
            processes: HashSet::new(),
        }
    }

    // ANCHOR_END: state

    // ANCHOR: behaviour
    fn broadcast_processess(&self) -> () {
        let procs: Vec<ActorPath> = self.processes.iter().cloned().collect();
        let msg = UpdateProcesses(procs);
        self.processes.iter().for_each(|process| {
            process.tell((msg.clone(), Serde), self);
        });
    }
}
ignore_lifecycle!(BootstrapServer);
impl NetworkActor for BootstrapServer {
    type Deserialiser = Serde;
    type Message = CheckIn;

    fn receive(&mut self, source: Option<ActorPath>, _msg: Self::Message) -> Handled {
        if let Some(process) = source {
            if self.processes.insert(process) {
                self.broadcast_processess();
            }
        }
        Handled::Ok
    }
}
// ANCHOR_END: behaviour

// ANCHOR: ele_state
#[derive(ComponentDefinition)]
struct EventualLeaderElector {
    ctx: ComponentContext<Self>,
    omega_port: ProvidedPort<EventualLeaderDetection>,
    bootstrap_server: ActorPath,
    processes: Box<[ActorPath]>,
    candidates: HashSet<ActorPath>,
    period: Duration,
    delta: Duration,
    timer_handle: Option<ScheduledTimer>,
    leader: Option<ActorPath>,
}
impl EventualLeaderElector {
    fn new(bootstrap_server: ActorPath) -> Self {
        let minimal_period = Duration::from_millis(1);
        EventualLeaderElector {
            ctx: ComponentContext::uninitialised(),
            omega_port: ProvidedPort::uninitialised(),
            bootstrap_server,
            processes: Vec::new().into_boxed_slice(),
            candidates: HashSet::new(),
            period: minimal_period,
            delta: minimal_period,
            timer_handle: None,
            leader: None,
        }
    }

    // ANCHOR_END: ele_state

    fn select_leader(&mut self) -> Option<ActorPath> {
        let mut candidates: Vec<ActorPath> = self.candidates.drain().collect();
        candidates.sort_unstable();
        candidates.reverse(); // pick smallest instead of largest
        candidates.pop()
    }

    fn handle_timeout(&mut self, timeout_id: ScheduledTimer) -> Handled {
        match self.timer_handle.take() {
            Some(timeout) if timeout == timeout_id => {
                let new_leader = self.select_leader();
                if new_leader != self.leader {
                    self.period += self.delta;
                    self.leader = new_leader;
                    if let Some(ref leader) = self.leader {
                        self.omega_port.trigger(Trust(leader.clone()));
                    }
                    self.cancel_timer(timeout);
                    let new_timer =
                        self.schedule_periodic(self.period, self.period, Self::handle_timeout);
                    self.timer_handle = Some(new_timer);
                } else {
                    // just put it back
                    self.timer_handle = Some(timeout);
                }
                self.send_heartbeats();
                Handled::Ok
            }
            Some(_) => Handled::Ok, // just ignore outdated timeouts
            None => {
                warn!(self.log(), "Got unexpected timeout: {:?}", timeout_id);
                Handled::Ok
            } // can happen during restart or teardown
        }
    }

    fn send_heartbeats(&self) {
        self.processes.iter().for_each(|process| {
            process.tell((Heartbeat, Serde), self);
        });
    }
}

impl ComponentLifecycle for EventualLeaderElector {
    fn on_start(&mut self) -> Handled {
        // ANCHOR: checkin
        self.bootstrap_server.tell((CheckIn, Serde), self);
        // ANCHOR_END: checkin

        self.period = self.ctx.config()["omega"]["initial-period"]
            .as_duration()
            .expect("initial period");
        self.delta = self.ctx.config()["omega"]["delta"]
            .as_duration()
            .expect("delta");
        let timeout = self.schedule_periodic(self.period, self.period, Self::handle_timeout);
        self.timer_handle = Some(timeout);
        Handled::Ok
    }

    fn on_stop(&mut self) -> Handled {
        if let Some(timeout) = self.timer_handle.take() {
            self.cancel_timer(timeout);
        }
        Handled::Ok
    }

    fn on_kill(&mut self) -> Handled {
        self.on_stop()
    }
}

// Doesn't have any requests
ignore_requests!(EventualLeaderDetection, EventualLeaderElector);

// ANCHOR: actor
impl Actor for EventualLeaderElector {
    type Message = Never;

    fn receive_local(&mut self, _msg: Self::Message) -> Handled {
        unreachable!();
    }

    fn receive_network(&mut self, msg: NetMessage) -> Handled {
        let sender = msg.sender;

        match_deser! {
            (msg.data) {
                msg(_heartbeat): Heartbeat [using Serde] => {
                    self.candidates.insert(sender);
                },
                msg(UpdateProcesses(processes)): UpdateProcesses [using Serde] => {
                    info!(
                        self.log(),
                        "Received new process set with {} processes",
                        processes.len()
                    );
                    self.processes = processes.into_boxed_slice();
                },
            }
        };
        Handled::Ok
    }
}
// ANCHOR_END: actor

// ANCHOR: main
pub fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.len() {
        2 => {
            let bootstrap_port: u16 = args[1].parse().expect("port number");
            let bootstrap_socket = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), bootstrap_port);
            let system = run_server(bootstrap_socket);
            system.await_termination(); // gotta quit it from command line
        }
        3 => {
            let bootstrap_port: u16 = args[1].parse().expect("port number");
            let bootstrap_socket = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), bootstrap_port);
            let client_port: u16 = args[2].parse().expect("port number");
            let client_socket = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), client_port);
            let system = run_client(bootstrap_socket, client_socket);
            system.await_termination(); // gotta quit it from command line
        }
        x => panic!("Expected either 1 argument (the port for the bootstrap server to bind on) or 2 arguments (boostrap server and client port), but got {} instead!", x-1),
    }
}
// ANCHOR_END: main

// ANCHOR: server
const BOOTSTRAP_PATH: &str = "bootstrap";

pub fn run_server(socket: SocketAddr) -> KompactSystem {
    let mut cfg = KompactConfig::new();
    cfg.load_config_file("./application.conf");
    cfg.system_components(DeadletterBox::new, NetworkConfig::new(socket).build());

    let system = cfg.build().expect("KompactSystem");

    let (bootstrap, bootstrap_registration) = system.create_and_register(BootstrapServer::new);
    let bootstrap_service_registration = system.register_by_alias(&bootstrap, BOOTSTRAP_PATH);

    let _bootstrap_unique = bootstrap_registration
        .wait_expect(Duration::from_millis(1000), "bootstrap never registered");
    let bootstrap_service = bootstrap_service_registration
        .wait_expect(Duration::from_millis(1000), "bootstrap never registered");
    system.start(&bootstrap);

    let printer = system.create(TrustPrinter::new);
    let (detector, registration) =
        system.create_and_register(|| EventualLeaderElector::new(bootstrap_service));
    biconnect_components::<EventualLeaderDetection, _, _>(&detector, &printer).expect("connection");
    let _path = registration.wait_expect(Duration::from_millis(1000), "detector never registered");
    system.start(&printer);
    system.start(&detector);

    system
}
// ANCHOR_END: server

// ANCHOR: client
pub fn run_client(bootstrap_socket: SocketAddr, client_socket: SocketAddr) -> KompactSystem {
    let mut cfg = KompactConfig::new();
    cfg.load_config_file("./application.conf");
    cfg.system_components(
        DeadletterBox::new,
        NetworkConfig::new(client_socket).build(),
    );

    let system = cfg.build().expect("KompactSystem");

    let bootstrap_service: ActorPath = NamedPath::with_socket(
        Transport::Tcp,
        bootstrap_socket,
        vec![BOOTSTRAP_PATH.into()],
    )
    .into();

    let printer = system.create(TrustPrinter::new);
    let (detector, registration) =
        system.create_and_register(|| EventualLeaderElector::new(bootstrap_service));
    biconnect_components::<EventualLeaderDetection, _, _>(&detector, &printer).expect("connection");
    let _path = registration.wait_expect(Duration::from_millis(1000), "detector never registered");
    system.start(&printer);
    system.start(&detector);

    system
}
// ANCHOR_END: client

#[cfg(test)]
mod tests {
    use super::*;

    const SERVER_SOCKET: &str = "127.0.0.1:12345";
    const CLIENT_SOCKET: &str = "127.0.0.1:0";

    #[test]
    fn test_bootstrapping() {
        let server_socket: SocketAddr = SERVER_SOCKET.parse().unwrap();
        let server_system = run_server(server_socket);
        let client_socket: SocketAddr = CLIENT_SOCKET.parse().unwrap();
        let mut clients_systems: Vec<KompactSystem> = (0..3)
            .map(|_i| run_client(server_socket, client_socket))
            .collect();
        // let them settle
        std::thread::sleep(Duration::from_millis(1000));
        // shut down systems one by one
        for sys in clients_systems.drain(..) {
            std::thread::sleep(Duration::from_millis(1000));
            sys.shutdown().expect("shutdown");
        }
        std::thread::sleep(Duration::from_millis(1000));
        server_system.shutdown().expect("shutdown");
    }
}

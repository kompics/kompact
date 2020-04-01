use kompact::{prelude::*, serde_serialisers::*};
use kompact_examples::trusting::*;
use std::{
    collections::HashSet,
    convert::TryInto,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::Duration,
};

// ANCHOR: zstser
struct ZSTSerialiser<T>(T)
    where
        T: Send + Sync + Default + Copy + SerialisationId;

impl<T> Serialiser<T> for &ZSTSerialiser<T>
    where
        T: Send + Sync + Default + Copy + SerialisationId,
{
    fn ser_id(&self) -> SerId {
        T::SER_ID
    }

    fn size_hint(&self) -> Option<usize> {
        Some(0)
    }

    fn serialise(&self, _v: &T, _buf: &mut dyn BufMut) -> Result<(), SerError> {
        Ok(())
    }
}

impl<T> Deserialiser<T> for ZSTSerialiser<T>
    where
        T: Send + Sync + Default + Copy + SerialisationId,
{
    const SER_ID: SerId = T::SER_ID;

    fn deserialise(_buf: &mut dyn Buf) -> Result<T, SerError> {
        Ok(T::default())
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct CheckIn;

impl SerialisationId for CheckIn {
    const SER_ID: SerId = 2345;
}

static CHECK_IN_SER: ZSTSerialiser<CheckIn> = ZSTSerialiser(CheckIn);
// ANCHOR_END: zstser

// ANCHOR: serialisable
#[derive(Debug, Clone)]
struct UpdateProcesses(Vec<ActorPath>);

impl Serialisable for UpdateProcesses {
    fn ser_id(&self) -> SerId {
        Self::SER_ID
    }

    fn size_hint(&self) -> Option<usize> {
        Some(8)
    }

    fn serialise(&self, buf: &mut dyn BufMut) -> Result<(), SerError> {
        let len = self.0.len() as u64;
        buf.put_u64(len);
        for path in self.0.iter() {
            path.serialise(buf)?;
        }
        Ok(())
    }

    fn local(self: Box<Self>) -> Result<Box<dyn Any + Send>, Box<dyn Serialisable>> {
        Ok(self)
    }
}

impl Deserialiser<UpdateProcesses> for UpdateProcesses {
    const SER_ID: SerId = 3456;

    fn deserialise(buf: &mut dyn Buf) -> Result<UpdateProcesses, SerError> {
        let len_u64 = buf.get_u64();
        let len: usize = len_u64.try_into().map_err(SerError::from_debug)?;
        let mut data: Vec<ActorPath> = Vec::with_capacity(len);
        for _i in 0..len {
            let path = ActorPath::deserialise(buf)?;
            data.push(path);
        }
        Ok(UpdateProcesses(data))
    }
}
// ANCHOR_END: serialisable

#[derive(ComponentDefinition)]
struct BootstrapServer {
    ctx: ComponentContext<Self>,
    processes: HashSet<ActorPath>,
}

impl BootstrapServer {
    fn new() -> Self {
        BootstrapServer {
            ctx: ComponentContext::new(),
            processes: HashSet::new(),
        }
    }

    // ANCHOR: tell_serialised
    fn broadcast_processess(&self) -> () {
        let procs: Vec<ActorPath> = self.processes.iter().cloned().collect();
        let msg = UpdateProcesses(procs);

        self.processes.iter().for_each(|process| {
            process
                .tell_serialised(msg.clone(), self)
                .unwrap_or_else(|e| warn!(self.log(), "Error during serialisation: {}", e));
        });
    }
    // ANCHOR_END: tell_serialised
}

//ignore_control!(BootstrapServer);
impl Provide<ControlPort> for BootstrapServer { fn handle(&mut self, _: ControlEvent) -> () {} }

impl NetworkActor for BootstrapServer {
    type Deserialiser = ZSTSerialiser<CheckIn>;
    type Message = CheckIn;

    fn receive(&mut self, source: Option<ActorPath>, _msg: Self::Message) -> () {
        if let Some(process) = source {
            if self.processes.insert(process) {
                self.broadcast_processess();
            }
        }
    }
}

#[derive(ComponentDefinition)]
struct EventualLeaderElector {
    ctx: ComponentContext<Self>,
    omega_port: ProvidedPort<EventualLeaderDetection, Self>,
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
            ctx: ComponentContext::new(),
            omega_port: ProvidedPort::new(),
            bootstrap_server,
            processes: Vec::new().into_boxed_slice(),
            candidates: HashSet::new(),
            period: minimal_period,
            delta: minimal_period,
            timer_handle: None,
            leader: None,
        }
    }

    fn select_leader(&mut self) -> Option<ActorPath> {
        let mut candidates: Vec<ActorPath> = self.candidates.drain().collect();
        candidates.sort_unstable();
        candidates.reverse(); // pick smallest instead of largest
        candidates.pop()
    }

    fn handle_timeout(&mut self, timeout_id: ScheduledTimer) -> () {
        match self.timer_handle.take() {
            Some(timeout) if timeout == timeout_id => {
                let new_leader = self.select_leader();
                if new_leader != self.leader {
                    self.period = self.period + self.delta;
                    self.leader = new_leader;
                    if let Some(ref leader) = self.leader {
                        self.omega_port.trigger(Trust(leader.clone()));
                    }
                    self.cancel_timer(timeout);
                    let new_timer = self.schedule_periodic(
                        self.period,
                        self.period,
                        EventualLeaderElector::handle_timeout,
                    );
                    self.timer_handle = Some(new_timer);
                } else {
                    // just put it back
                    self.timer_handle = Some(timeout);
                }
                self.send_heartbeats();
            }
            Some(_) => (), // just ignore outdated timeouts
            None => warn!(self.log(), "Got unexpected timeout: {:?}", timeout_id), // can happen during restart or teardown
        }
    }

    // ANCHOR: send_heartbeats
    fn send_heartbeats(&self) -> () {
        self.processes.iter().for_each(|process| {
            process.tell((Heartbeat, Serde), self);
        });
    }
    // ANCHOR_END: send_heartbeats
}

impl Provide<ControlPort> for EventualLeaderElector {
    fn handle(&mut self, event: ControlEvent) -> () {
        match event {
            ControlEvent::Start => {
                // ANCHOR: tell_checkin
                self.bootstrap_server.tell((CheckIn, &CHECK_IN_SER), self);
                // ANCHOR_END: tell_checkin

                self.period = self.ctx.config()["omega"]["initial-period"]
                    .as_duration()
                    .expect("initial period");
                self.delta = self.ctx.config()["omega"]["delta"]
                    .as_duration()
                    .expect("delta");
                let timeout = self.schedule_periodic(
                    self.period,
                    self.period,
                    EventualLeaderElector::handle_timeout,
                );
                self.timer_handle = Some(timeout);
            }
            ControlEvent::Stop | ControlEvent::Kill => {
                if let Some(timeout) = self.timer_handle.take() {
                    self.cancel_timer(timeout);
                }
            }
        }
    }
}
// Doesn't have any requests
ignore_requests!(EventualLeaderDetection, EventualLeaderElector);

impl Actor for EventualLeaderElector {
    type Message = Never;

    fn receive_local(&mut self, _msg: Self::Message) -> () {
        unreachable!();
    }

    // ANCHOR: receive_network
    fn receive_network(&mut self, msg: NetMessage) -> () {
        let sender = msg.sender;

        match_deser!(msg.data; {
            _heartbeat: Heartbeat [Serde] => {
                self.candidates.insert(sender);
            },
            update: UpdateProcesses [UpdateProcesses] => {
                let UpdateProcesses(processes) = update;
                info!(
                    self.log(),
                    "Received new process set with {} processes",
                    processes.len()
                );
                self.processes = processes.into_boxed_slice();
            },
        });
    }
    // ANCHOR_END: receive_network
}

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
        x => panic!("Expected either 1 argument (the port for the bootstrap server to bind on) or 2 arguments (boostrap server and client port), but got {} instead!", x - 1),
    }
}

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

pub fn run_client(bootstrap_socket: SocketAddr, client_socket: SocketAddr) -> KompactSystem {
    let mut cfg = KompactConfig::new();
    cfg.load_config_file("./application.conf");
    cfg.system_components(
        DeadletterBox::new,
        NetworkConfig::new(client_socket).build(),
    );

    let system = cfg.build().expect("KompactSystem");

    let bootstrap_service: ActorPath = NamedPath::with_socket(
        Transport::TCP,
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

#[cfg(test)]
mod tests {
    use super::*;

    const SERVER_SOCKET: &str = "127.0.0.1:12345";
    const CLIENT_SOCKET: &str = "127.0.0.1:0";

    #[test]
    fn test_bootstrapping_serialisation() {
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

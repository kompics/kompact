use kompact::{prelude::*, serde_serialisers::*};
use kompact_examples::trusting::*;
use std::{collections::HashSet, sync::Arc, time::Duration};

#[derive(Debug)]
struct UpdateProcesses(Arc<[ActorPath]>);

#[derive(ComponentDefinition)]
struct EventualLeaderElector {
    ctx: ComponentContext<Self>,
    omega_port: ProvidedPort<EventualLeaderDetection, Self>,
    processes: Arc<[ActorPath]>,
    candidates: HashSet<ActorPath>,
    period: Duration,
    delta: Duration,
    timer_handle: Option<ScheduledTimer>,
    leader: Option<ActorPath>,
}
impl EventualLeaderElector {
    fn new() -> Self {
        let minimal_period = Duration::from_millis(1);
        EventualLeaderElector {
            ctx: ComponentContext::new(),
            omega_port: ProvidedPort::new(),
            processes: Vec::new().into_boxed_slice().into(),
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

    fn send_heartbeats(&self) -> () {
        self.processes.iter().for_each(|process| {
            process.tell((Heartbeat, Serde), self);
        });
    }
}

impl Provide<ControlPort> for EventualLeaderElector {
    fn handle(&mut self, event: ControlEvent) -> () {
        match event {
            ControlEvent::Start => {
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
    type Message = UpdateProcesses;

    fn receive_local(&mut self, msg: Self::Message) -> () {
        info!(
            self.log(),
            "Received new process set with {} processes",
            msg.0.len()
        );
        let UpdateProcesses(processes) = msg;
        self.processes = processes;
    }

    fn receive_network(&mut self, msg: NetMessage) -> () {
        let sender = msg.sender().clone();
        match msg.try_deserialise::<Heartbeat, Serde>() {
            Ok(_heartbeat) => {
                self.candidates.insert(sender);
            }
            Err(e) => warn!(self.log(), "Invalid data: {:?}", e),
        }
    }
}

pub fn main() {
    let args: Vec<String> = std::env::args().collect();
    assert_eq!(
        2,
        args.len(),
        "Invalid arguments! Must give number of systems."
    );
    let num_systems: usize = args[1].parse().expect("number");
    run_systems(num_systems);
}

pub fn run_systems(num_systems: usize) {
    let mut systems: Vec<KompactSystem> = {
        let system = || {
            let mut cfg = KompactConfig::new();
            cfg.load_config_file("./application.conf");
            cfg.system_components(DeadletterBox::new, NetworkConfig::default().build());
            cfg.build().expect("KompactSystem")
        };
        let mut data = Vec::with_capacity(num_systems);
        for _i in 0..num_systems {
            let sys = system();
            data.push(sys);
        }
        data
    };

    let (processes, actors): (Vec<ActorPath>, Vec<ActorRef<UpdateProcesses>>) = systems
        .iter()
        .map(|sys| {
            let printer = sys.create(TrustPrinter::new);
            let (detector, registration) = sys.create_and_register(EventualLeaderElector::new);
            biconnect_components::<EventualLeaderDetection, _, _>(&detector, &printer)
                .expect("connection");
            let path =
                registration.wait_expect(Duration::from_millis(1000), "actor never registered");
            sys.start(&printer);
            sys.start(&detector);
            (path, detector.actor_ref())
        })
        .unzip();

    let shared_processes: Arc<[ActorPath]> = processes.into_boxed_slice().into();

    actors.iter().for_each(|actor| {
        let update = UpdateProcesses(shared_processes.clone());
        actor.tell(update);
    });
    // let them settle
    std::thread::sleep(Duration::from_millis(1000));
    // shut down systems one by one
    for sys in systems.drain(..) {
        std::thread::sleep(Duration::from_millis(1000));
        sys.shutdown().expect("shutdown");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_omega() {
        run_systems(3);
    }
}

use kompact::prelude::*;
use kompact::{prelude::*, serde_serialisers::*};
use serde::{Deserialize, Serialize};

use std::{
    time::Duration,
    thread::{
        current,
    }
};

#[derive(ComponentDefinition)]
struct Pinger {
    ctx: ComponentContext<Self>,
    actor_path: ActorPath,
}

#[derive(ComponentDefinition)]
struct Ponger {
    ctx: ComponentContext<Self>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
struct Ping;
impl SerialisationId for Ping {
    const SER_ID: SerId = 1234;
}
#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
struct Pong;
impl SerialisationId for Pong {
    const SER_ID: SerId = 4321;
}

struct PingPongPort;
impl Port for PingPongPort {
    type Indication = Pong;
    type Request = Ping;
}

impl Pinger {
    pub fn new(actor_path:ActorPath) -> Self {
        Pinger {
            ctx: ComponentContext::uninitialised(),
            actor_path: actor_path,
        }
    }
}

impl Ponger {
    pub fn new() -> Self {
        Ponger {
            ctx: ComponentContext::uninitialised(),
        }
    }
}

impl ComponentLifecycle for Pinger {
    fn on_start(&mut self) -> Handled {
        println!("On start Pinger");
        info!(self.log(), "Pinger started!");
        self.actor_path.tell((Ping, Serde), self);
        Handled::Ok
    }
}

//redundant, but just to log
impl ComponentLifecycle for Ponger {
    fn on_start(&mut self) -> Handled {
        println!("On start Ponger");
        info!(self.log(), "Ponger started!");
        Handled::Ok
    }
}

impl Actor for Pinger {
    type Message = Never;

    fn receive_local(&mut self, msg: Self::Message) -> Handled {
        unimplemented!("We are ignoring local messages");
    }

    fn receive_network(&mut self, msg: NetMessage) -> Handled {
        let sender = msg.sender;
        match msg.data.try_deserialise::<Pong, Serde>() {
            Ok(_ping) => {
                info!(self.log(), "RECIEVED PONG IN PINGER");
                sender.tell((Ping, Serde), self)
            }
            Err(e) => warn!(self.log(), "Invalid data: {:?}", e),
        }
        Handled::Ok
    }
}

impl Actor for Ponger {
    type Message = Never;

    fn receive_local(&mut self, msg: Self::Message) -> Handled {
        unimplemented!("We are ignoring local messages");
    }

    fn receive_network(&mut self, msg: NetMessage) -> Handled {
        let sender = msg.sender;
        match msg.data.try_deserialise::<Ping, Serde>() {
            Ok(_ping) => {
                info!(self.log(), "RECIEVED PING IN PONGER");
                sender.tell((Pong, Serde), self)
            }
            Err(e) => warn!(self.log(), "Invalid data: {:?}", e),
        }
        Handled::Ok
    }
}

pub fn sim() {
    let mut simulation = SimulationScenario::new();

    let mut cfg2 = KompactConfig::default();
    let sys2 = simulation.spawn_system(cfg2);

    let mut cfg1 = KompactConfig::default();
    let sys1 = simulation.spawn_system(cfg1);

    let (ponger, registration_future) = sys2.create_and_register(Ponger::new);
    let path = registration_future.wait_expect(Duration::from_millis(1000), "actor never registered");

    let (pinger, registration_future) = sys1.create_and_register(move || Pinger::new(path));
    registration_future.wait_expect(Duration::from_millis(1000), "actor never registered");

    simulation.end_setup();
    
    sys2.start(&ponger);
    println!("start in main");
    sys1.start(&pinger);
    println!("start in main 2");
    
    /* 
    println!("start in main 1 {:?}", simulation.simulate_step());
    println!("start in main 2 {:?}", simulation.simulate_step());
    println!("start in main 3 {:?}", simulation.simulate_step());
    println!("start in main 4 {:?}", simulation.simulate_step());
    println!("start in main 5 {:?}", simulation.simulate_step());
    println!("start in main 6 {:?}", simulation.simulate_step());
    println!("start in main 7 {:?}", simulation.simulate_step());
    println!("start in main 8 {:?}", simulation.simulate_step());
    println!("start in main 9 {:?}", simulation.simulate_step());
    println!("start in main 10 {:?}", simulation.simulate_step());
    println!("start in main 11 {:?}", simulation.simulate_step());
    println!("start in main 12 {:?}", simulation.simulate_step());
    println!("start in main 13 {:?}", simulation.simulate_step());
    println!("start in main 14 {:?}", simulation.simulate_step());
    println!("start in main 15 {:?}", simulation.simulate_step());
    println!("start in main 16 {:?}", simulation.simulate_step());
    println!("start in main 17 {:?}", simulation.simulate_step());
    println!("start in main 18 {:?}", simulation.simulate_step());
    println!("start in main 19 {:?}", simulation.simulate_step());
    */

    simulation.simulate_to_completion();

    std::thread::sleep(Duration::from_millis(1000));
    sys1.await_termination(); //shutdown().expect("shutdown");
    println!("shutdown");

    sys2.await_termination(); //.shutdown().expect("shutdown");
    println!("shutdown 2");
} 



pub fn nonsim() {
    let mut cfg2 = KompactConfig::default();
    cfg2.system_components(DeadletterBox::new, NetworkConfig::default().build());
    let sys2 = cfg2.build().expect("sys2");

    let mut cfg1 = KompactConfig::default();
    cfg1.system_components(DeadletterBox::new, NetworkConfig::default().build());
    let sys1 = cfg1.build().expect("sys1");

    let (ponger, registration_future) = sys2.create_and_register(Ponger::new);
    let path = registration_future.wait_expect(Duration::from_millis(1000), "actor never registered");

    let (pinger, registration_future) = sys1.create_and_register(move || Pinger::new(path));
    registration_future.wait_expect(Duration::from_millis(1000), "actor never registered");
    
    sys2.start(&ponger);
    sys1.start(&pinger);

    std::thread::sleep(Duration::from_millis(1000));
    sys1.shutdown().expect("shutdown");
    sys2.shutdown().expect("shutdown");
}

pub fn main(){
    sim();
}

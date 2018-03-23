extern crate kompics;
//#[macro_use]
//extern crate component_definition_derive;

use kompics::*;
use std::sync::Arc;
use std::{thread, time};

#[derive(Clone, Debug)]
struct Ping;
#[derive(Clone, Debug)]
struct Pong;

struct PingPongPort;

impl Port for PingPongPort {
    type Indication = Pong;
    type Request = Ping;
}

#[derive(ComponentDefinition)]
struct Pinger {
    ppp: RequiredPort<PingPongPort, Pinger>,
    pppp: ProvidedPort<PingPongPort, Pinger>,
    test: i32,
}

impl Pinger {
    fn new() -> Pinger {
        Pinger {
            ppp: RequiredPort::new(),
            pppp: ProvidedPort::new(),
            test: 0,
        }
    }
}

impl Provide<ControlPort> for Pinger {
    fn handle(&mut self, event: ControlEvent) -> () {
        match event {
            ControlEvent::Start => {
                println!("Starting Pinger... {}", self.test);
            }
            _ => (),// ignore
        }
    }
}

impl Require<PingPongPort> for Pinger {
    fn handle(&mut self, event: Pong) -> () {
        println!("Got a pong!");
    }
}

impl Provide<PingPongPort> for Pinger {
    fn handle(&mut self, event: Ping) -> () {
        println!("Got a ping!");
    }
}

fn main() {
    println!("Hello, world!");
    //let cd = SomeCD { test: 3 };
    let mut conf = KompicsConfig::new();
    {
        conf.throughput(5);
    }
    let system = KompicsSystem::new(conf);
    let mut pingerc = system.create(move || Pinger::new());
    system.start(&pingerc);
    system.trigger_i(Pong, pingerc.on_definition(|cd| cd.ppp.share()));
    thread::sleep(time::Duration::from_millis(5000));
}

#![allow(unused_parens)]

use kompact::prelude::*;
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

#[derive(ComponentDefinition, Actor)]
struct Pinger {
    ctx: ComponentContext<Pinger>,
    ppp: RequiredPort<PingPongPort>,
    pppp: ProvidedPort<PingPongPort>,
    test: i32,
}

impl Pinger {
    fn new() -> Pinger {
        Pinger {
            ctx: ComponentContext::uninitialised(),
            ppp: RequiredPort::uninitialised(),
            pppp: ProvidedPort::uninitialised(),
            test: 0,
        }
    }
}

impl ComponentLifecycle for Pinger {
    fn on_start(&mut self) -> Handled {
        println!("Starting Pinger... {}", self.test);
        Handled::Ok
    }
}

impl Require<PingPongPort> for Pinger {
    fn handle(&mut self, _event: Pong) -> Handled {
        println!("Got a pong!");
        Handled::Ok
    }
}

impl Provide<PingPongPort> for Pinger {
    fn handle(&mut self, _event: Ping) -> Handled {
        println!("Got a ping!");
        Handled::Ok
    }
}

// Tests that the macro allows multiple ports
#[derive(ComponentDefinition, Actor)]
struct Pinger2 {
    ctx: ComponentContext<Pinger2>,
    ppp: RequiredPort<PingPongPort>,
    ppp2: RequiredPort<PingPongPort2>,
    pppp: ProvidedPort<PingPongPort>,
    test: i32,
}

struct PingPongPort2;

impl Port for PingPongPort2 {
    type Indication = Pong;
    type Request = Ping;
}

impl ComponentLifecycle for Pinger2 {
    fn on_start(&mut self) -> Handled {
        println!("Starting Pinger... {}", self.test);
        Handled::Ok
    }
}

impl Require<PingPongPort> for Pinger2 {
    fn handle(&mut self, _event: Pong) -> Handled {
        println!("Got a pong!");
        Handled::Ok
    }
}

impl Require<PingPongPort2> for Pinger2 {
    fn handle(&mut self, _event: Pong) -> Handled {
        println!("Got a pong!");
        Handled::Ok
    }
}

impl Provide<PingPongPort> for Pinger2 {
    fn handle(&mut self, _event: Ping) -> Handled {
        println!("Got a ping!");
        Handled::Ok
    }
}

#[derive(ComponentDefinition)]
pub struct GenericComp<A: MessageBounds> {
    ctx: ComponentContext<Self>,
    test: Option<A>,
}

impl<A: MessageBounds> GenericComp<A> {
    fn new() -> Self {
        GenericComp {
            ctx: ComponentContext::uninitialised(),
            test: None,
        }
    }
}

impl<A: MessageBounds> ComponentLifecycle for GenericComp<A> {}

impl<A: MessageBounds> Actor for GenericComp<A> {
    type Message = A;

    fn receive_local(&mut self, msg: Self::Message) -> Handled {
        self.test = Some(msg);
        Handled::Ok
    }

    fn receive_network(&mut self, _msg: NetMessage) -> Handled {
        Handled::Ok
    }
}

fn main() {
    let mut conf = KompactConfig::new();
    {
        conf.throughput(5);
    }
    let system = conf.build().expect("KompactSystem");
    let pingerc = system.create(move || Pinger::new());
    let pinger_ppp: RequiredRef<PingPongPort> = pingerc.required_ref(); //pingerc.on_definition(|cd| cd.ppp.share());
    system
        .start_notify(&pingerc)
        .wait_timeout(time::Duration::from_millis(100))
        .expect("started");
    system.trigger_i(Pong, &pinger_ppp);

    // thread::sleep(time::Duration::from_millis(5000));

    // let generic_comp = system.create_and_start(move || {
    //     let g: GenericComp<String> = GenericComp::new();
    //     g
    // });
    let generic_comp = system.create(GenericComp::<String>::new);
    system
        .start_notify(&generic_comp)
        .wait_timeout(time::Duration::from_millis(100))
        .expect("started");
    let msg = String::from("Test");
    generic_comp.actor_ref().tell(msg.clone());
    thread::sleep(time::Duration::from_millis(100));
    //let comp_inspect = &generic_comp.definition().lock().unwrap();
    generic_comp.on_definition(|cd| match cd.test {
        Some(ref test) => assert_eq!(test, &msg),
        None => panic!("test should have been Some"),
    });
}

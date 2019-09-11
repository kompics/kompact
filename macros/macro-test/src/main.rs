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
    ppp: RequiredPort<PingPongPort, Pinger>,
    pppp: ProvidedPort<PingPongPort, Pinger>,
    test: i32,
}

impl Pinger {
    fn new() -> Pinger {
        Pinger {
            ctx: ComponentContext::new(),
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
            _ => (), // ignore
        }
    }
}

impl Require<PingPongPort> for Pinger {
    fn handle(&mut self, _event: Pong) -> () {
        println!("Got a pong!");
    }
}

impl Provide<PingPongPort> for Pinger {
    fn handle(&mut self, _event: Ping) -> () {
        println!("Got a ping!");
    }
}

#[derive(ComponentDefinition)]
pub struct GenericComp<A>
where
    A: 'static + Sync + Send + Clone,
{
    ctx: ComponentContext<Self>,
    test: Option<A>,
}

impl<A> GenericComp<A>
where
    A: 'static + Sync + Send + Clone,
{
    fn new() -> Self {
        GenericComp {
            ctx: ComponentContext::new(),
            test: None,
        }
    }
}

impl<A> Provide<ControlPort> for GenericComp<A>
where
    A: 'static + Sync + Send + Clone,
{
    fn handle(&mut self, _event: ControlEvent) -> () {}
}

impl<A> Actor for GenericComp<A>
where
    A: 'static + Sync + Send + Clone,
{
    fn receive_local(&mut self, _sender: ActorRef, msg: &dyn Any) {
        if let Some(event) = msg.downcast_ref::<A>() {
            self.test = Some(event.clone());
        }
    }

    fn receive_message(&mut self, _sender: ActorPath, _ser_id: u64, _buf: &mut dyn Buf) {}
}

fn main() {
    let mut conf = KompactConfig::new();
    {
        conf.throughput(5);
    }
    let system = conf.build().expect("KompactSystem");
    let pingerc = system.create(move || Pinger::new());
    system.start(&pingerc);
    system.trigger_i(Pong, &pingerc.on_definition(|cd| cd.ppp.share()));
    thread::sleep(time::Duration::from_millis(5000));

    let generic_comp = system.create_and_start(move || {
        let g: GenericComp<String> = GenericComp::new();
        g
    });
    thread::sleep(time::Duration::from_millis(100));
    generic_comp
        .actor_ref()
        .tell(Box::new(String::from("Test")), &generic_comp);
    thread::sleep(time::Duration::from_millis(100));
    let comp_inspect = &generic_comp.definition().lock().unwrap();
    assert_eq!(comp_inspect.test.as_ref().unwrap(), &String::from("Test"));
}

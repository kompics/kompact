use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use kompact::prelude::*;
use std::{fmt, sync::Arc};
use synchronoise::CountdownEvent;

// #[inline(always)]
// pub fn ignore<V>(_: V) -> () {
//     ()
// }

const MSG_COUNT: u64 = 50000;

pub fn ping_benches(c: &mut Criterion) {
    let mut g = c.benchmark_group("Ping Benches");
    for pairs in [1usize, 2usize, 4usize, 8usize, 16usize].iter() {
        g.throughput(Throughput::Elements(2u64 * MSG_COUNT * (*pairs as u64)));
        g.bench_with_input(
            BenchmarkId::new("ports", pairs),
            pairs,
            tests::ping_pong_throughput_ports,
        );
        g.bench_with_input(
            BenchmarkId::new("strong-refs", pairs),
            pairs,
            tests::ping_pong_throughput_strong_ref,
        );
        g.bench_with_input(
            BenchmarkId::new("weak-refs", pairs),
            pairs,
            tests::ping_pong_throughput_weak_ref,
        );
    }
    g.finish();
}

#[derive(Clone)]
pub struct Start(Arc<CountdownEvent>);

impl fmt::Debug for Start {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Start(<CountdownEvent>)")
    }
}

#[derive(Clone, Debug)]
pub struct Ping;
#[derive(Clone, Debug)]
pub struct Pong;

const MAX_BATCH: u64 = 1000;

mod port_pingpong {
    use super::*;

    pub struct PingPongPort;

    impl Port for PingPongPort {
        type Indication = Pong;
        type Request = Ping;
    }

    #[derive(ComponentDefinition)]
    pub struct Pinger {
        ctx: ComponentContext<Pinger>,
        ppp: RequiredPort<PingPongPort>,
        latch: Option<Arc<CountdownEvent>>,
        repeat: u64,
        sent: u64,
        received: u64,
    }

    impl Pinger {
        pub fn with(repeat: u64) -> Pinger {
            Pinger {
                ctx: ComponentContext::new(),
                ppp: RequiredPort::new(),
                latch: None,
                repeat,
                sent: 0,
                received: 0,
            }
        }
    }

    impl Provide<ControlPort> for Pinger {
        fn handle(&mut self, _event: ControlEvent) {
            // nothing
        }
    }

    impl Require<PingPongPort> for Pinger {
        fn handle(&mut self, _event: Pong) {
            self.received += 1;
            if self.sent < self.repeat {
                self.ppp.trigger(Ping);
                self.sent += 1;
            } else if self.received >= self.repeat {
                let _ = self.latch.take().unwrap().decrement();
            }
        }
    }

    impl Actor for Pinger {
        type Message = Start;

        fn receive_local(&mut self, msg: Self::Message) {
            self.sent = 0;
            self.received = 0;
            self.latch = Some(msg.0);
            let remaining = std::cmp::min(MAX_BATCH, self.repeat);
            for _ in 0..remaining {
                self.ppp.trigger(Ping);
                self.sent += 1;
            }
        }

        fn receive_network(&mut self, _msg: NetMessage) {
            unimplemented!("not needed");
        }
    }

    #[derive(ComponentDefinition, Actor)]
    pub struct Ponger {
        ctx: ComponentContext<Ponger>,
        ppp: ProvidedPort<PingPongPort>,
        received: u64,
    }

    impl Ponger {
        pub fn new() -> Ponger {
            Ponger {
                ctx: ComponentContext::new(),
                ppp: ProvidedPort::new(),
                received: 0,
            }
        }
    }

    impl Provide<ControlPort> for Ponger {
        fn handle(&mut self, _event: ControlEvent) {
            // nothing
        }
    }

    impl Provide<PingPongPort> for Ponger {
        fn handle(&mut self, _event: Ping) {
            self.received += 1;
            //println!("Got Ping #{}", self.received);
            self.ppp.trigger(Pong);
        }
    }
}

mod strong_ref_pingpong {
    use super::*;

    pub enum PingerMsg {
        Start {
            latch: Arc<CountdownEvent>,
            ponger: ActorRefStrong<Ping>,
        },
        Pong,
    }
    impl fmt::Debug for PingerMsg {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                PingerMsg::Start { .. } => f
                    .debug_struct("PingerMsg::Start")
                    .field("latch", &"<CountdownEvent>")
                    .field("ponger", &"<ActorRefStrong>")
                    .finish(),
                PingerMsg::Pong => f.write_str("PingerMsg::Pong"),
            }
        }
    }

    #[derive(ComponentDefinition)]
    pub struct Pinger {
        ctx: ComponentContext<Pinger>,
        ponger: Option<ActorRefStrong<Ping>>,
        latch: Option<Arc<CountdownEvent>>,
        repeat: u64,
        sent: u64,
        received: u64,
    }

    impl Pinger {
        pub fn with(repeat: u64) -> Pinger {
            Pinger {
                ctx: ComponentContext::new(),
                ponger: None,
                latch: None,
                repeat,
                sent: 0,
                received: 0,
            }
        }
    }

    impl Provide<ControlPort> for Pinger {
        fn handle(&mut self, _event: ControlEvent) {
            // nothing
        }
    }

    impl Actor for Pinger {
        type Message = PingerMsg;

        fn receive_local(&mut self, msg: Self::Message) {
            match msg {
                PingerMsg::Start { ponger, latch } => {
                    self.sent = 0;
                    self.received = 0;
                    self.latch = Some(latch);
                    let remaining = std::cmp::min(MAX_BATCH, self.repeat);
                    for _ in 0..remaining {
                        ponger.tell(Ping);
                        self.sent += 1;
                    }
                    self.ponger = Some(ponger);
                }
                PingerMsg::Pong => {
                    self.received += 1;
                    if self.sent < self.repeat {
                        if let Some(ref ponger) = self.ponger {
                            ponger.tell(Ping);
                            self.sent += 1;
                        } else {
                            unreachable!("Ponger should have been set here!");
                        }
                    } else if self.received >= self.repeat {
                        let _ = self.latch.take().unwrap().decrement();
                    }
                }
            }
        }

        fn receive_network(&mut self, _msg: NetMessage) {
            unimplemented!("not needed");
        }
    }

    #[derive(ComponentDefinition)]
    pub struct Ponger {
        ctx: ComponentContext<Ponger>,
        pinger: ActorRefStrong<PingerMsg>,
        received: u64,
    }

    impl Ponger {
        pub fn new(pinger: ActorRefStrong<PingerMsg>) -> Ponger {
            Ponger {
                ctx: ComponentContext::new(),
                pinger,
                received: 0,
            }
        }
    }

    impl Provide<ControlPort> for Ponger {
        fn handle(&mut self, _event: ControlEvent) {
            // nothing
        }
    }

    impl Actor for Ponger {
        type Message = Ping;

        fn receive_local(&mut self, _msg: Self::Message) {
            self.received += 1;
            self.pinger.tell(PingerMsg::Pong);
        }

        fn receive_network(&mut self, _msg: NetMessage) {
            unimplemented!("not needed");
        }
    }
}

mod weak_ref_pingpong {
    use super::*;

    pub enum PingerMsg {
        Start {
            latch: Arc<CountdownEvent>,
            ponger: ActorRef<Ping>,
        },
        Pong,
    }
    impl fmt::Debug for PingerMsg {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                PingerMsg::Start { .. } => f
                    .debug_struct("PingerMsg::Start")
                    .field("latch", &"<CountdownEvent>")
                    .field("ponger", &"<ActorRef>")
                    .finish(),
                PingerMsg::Pong => f.write_str("PingerMsg::Pong"),
            }
        }
    }

    #[derive(ComponentDefinition)]
    pub struct Pinger {
        ctx: ComponentContext<Pinger>,
        ponger: Option<ActorRef<Ping>>,
        latch: Option<Arc<CountdownEvent>>,
        repeat: u64,
        sent: u64,
        received: u64,
    }

    impl Pinger {
        pub fn with(repeat: u64) -> Pinger {
            Pinger {
                ctx: ComponentContext::new(),
                ponger: None,
                latch: None,
                repeat,
                sent: 0,
                received: 0,
            }
        }
    }

    impl Provide<ControlPort> for Pinger {
        fn handle(&mut self, _event: ControlEvent) {
            // nothing
        }
    }

    impl Actor for Pinger {
        type Message = PingerMsg;

        fn receive_local(&mut self, msg: Self::Message) {
            match msg {
                PingerMsg::Start { ponger, latch } => {
                    self.sent = 0;
                    self.received = 0;
                    self.latch = Some(latch);
                    let remaining = std::cmp::min(MAX_BATCH, self.repeat);
                    for _ in 0..remaining {
                        ponger.tell(Ping);
                        self.sent += 1;
                    }
                    self.ponger = Some(ponger);
                }
                PingerMsg::Pong => {
                    self.received += 1;
                    if self.sent < self.repeat {
                        if let Some(ref ponger) = self.ponger {
                            ponger.tell(Ping);
                            self.sent += 1;
                        } else {
                            unreachable!("Ponger should have been set here!");
                        }
                    } else if self.received >= self.repeat {
                        let _ = self.latch.take().unwrap().decrement();
                    }
                }
            }
        }

        fn receive_network(&mut self, _msg: NetMessage) {
            unimplemented!("not needed");
        }
    }

    #[derive(ComponentDefinition)]
    pub struct Ponger {
        ctx: ComponentContext<Ponger>,
        pinger: ActorRef<PingerMsg>,
        received: u64,
    }

    impl Ponger {
        pub fn new(pinger: ActorRef<PingerMsg>) -> Ponger {
            Ponger {
                ctx: ComponentContext::new(),
                pinger,
                received: 0,
            }
        }
    }

    impl Provide<ControlPort> for Ponger {
        fn handle(&mut self, _event: ControlEvent) {
            // nothing
        }
    }

    impl Actor for Ponger {
        type Message = Ping;

        fn receive_local(&mut self, _msg: Self::Message) {
            self.received += 1;
            self.pinger.tell(PingerMsg::Pong);
        }

        fn receive_network(&mut self, _msg: NetMessage) {
            unimplemented!("not needed");
        }
    }
}

mod tests {
    use super::*;
    use criterion::{BatchSize, Bencher};
    use std::time::Duration;

    fn setup_kompact_system() -> KompactSystem {
        KompactConfig::default().build().expect("KompactSystem")
    }

    pub fn ping_pong_throughput_ports(b: &mut Bencher, pairs: &usize) {
        use super::port_pingpong::*;
        let pairs = *pairs;
        let timeout = Duration::from_millis(1000);

        let system = setup_kompact_system();
        let mut pingers: Vec<Arc<Component<Pinger>>> = Vec::with_capacity(pairs);
        let mut start_refs: Vec<ActorRefStrong<Start>> = Vec::with_capacity(pairs);
        let mut pongers: Vec<Arc<Component<Ponger>>> = Vec::with_capacity(pairs);
        for _ in 0..pairs {
            let pingerc = system.create(|| Pinger::with(MSG_COUNT));
            let pongerc = system.create(Ponger::new);
            biconnect_components::<PingPongPort, _, _>(&pongerc, &pingerc)
                .expect("Could not connect a ping-pair!");
            start_refs.push(pingerc.actor_ref().hold().expect("actor ref"));
            let pinger_f = system.start_notify(&pingerc);
            let ponger_f = system.start_notify(&pongerc);
            pinger_f
                .wait_timeout(timeout)
                .expect("Pinger didn't start.");
            ponger_f
                .wait_timeout(timeout)
                .expect("Pinger didn't start.");
            pingers.push(pingerc);
            pongers.push(pongerc);
        }
        b.iter_batched_ref(
            || Arc::new(CountdownEvent::new(pairs)),
            |latch| {
                for pinger_ref in start_refs.iter() {
                    pinger_ref.tell(Start(latch.clone()));
                }
                latch.wait();
            },
            BatchSize::PerIteration,
        );

        let mut futures = Vec::with_capacity(2 * pairs);
        drop(start_refs);
        for pinger in pingers {
            let f = system.kill_notify(pinger);
            futures.push(f);
        }
        for ponger in pongers {
            let f = system.kill_notify(ponger);
            futures.push(f);
        }
        for f in futures {
            f.wait_timeout(timeout)
                .expect("Some component didn't shut down properly!");
        }
        system
            .shutdown()
            .expect("Kompact didn't shut down properly");
    }

    pub fn ping_pong_throughput_strong_ref(b: &mut Bencher, pairs: &usize) {
        use super::strong_ref_pingpong::*;
        let pairs = *pairs;
        let timeout = Duration::from_millis(1000);

        let system = setup_kompact_system();
        let mut pingers: Vec<Arc<Component<Pinger>>> = Vec::with_capacity(pairs);
        let mut start_refs: Vec<(ActorRefStrong<PingerMsg>, ActorRefStrong<Ping>)> =
            Vec::with_capacity(pairs);
        let mut pongers: Vec<Arc<Component<Ponger>>> = Vec::with_capacity(pairs);
        for _ in 0..pairs {
            let pingerc = system.create(|| Pinger::with(MSG_COUNT));
            let pinger_ref = pingerc.actor_ref().hold().expect("actor ref");
            let pongerc = system.create(|| Ponger::new(pinger_ref.clone()));
            let ponger_ref = pongerc.actor_ref().hold().expect("actor ref");
            start_refs.push((pinger_ref, ponger_ref));
            let pinger_f = system.start_notify(&pingerc);
            let ponger_f = system.start_notify(&pongerc);
            pinger_f
                .wait_timeout(timeout)
                .expect("Pinger didn't start.");
            ponger_f
                .wait_timeout(timeout)
                .expect("Pinger didn't start.");
            pingers.push(pingerc);
            pongers.push(pongerc);
        }
        b.iter_batched_ref(
            || Arc::new(CountdownEvent::new(pairs)),
            |latch| {
                for (pinger_ref, ponger_ref) in start_refs.iter() {
                    pinger_ref.tell(PingerMsg::Start {
                        latch: latch.clone(),
                        ponger: ponger_ref.clone(),
                    });
                }
                latch.wait();
            },
            BatchSize::PerIteration,
        );

        let mut futures = Vec::with_capacity(2 * pairs);
        drop(start_refs);
        for pinger in pingers {
            let f = system.kill_notify(pinger);
            futures.push(f);
        }
        for ponger in pongers {
            let f = system.kill_notify(ponger);
            futures.push(f);
        }
        for f in futures {
            f.wait_timeout(timeout)
                .expect("Some component didn't shut down properly!");
        }
        system
            .shutdown()
            .expect("Kompact didn't shut down properly");
    }

    pub fn ping_pong_throughput_weak_ref(b: &mut Bencher, pairs: &usize) {
        use super::weak_ref_pingpong::*;
        let pairs = *pairs;
        let timeout = Duration::from_millis(1000);

        let system = setup_kompact_system();
        let mut pingers: Vec<Arc<Component<Pinger>>> = Vec::with_capacity(pairs);
        let mut start_refs: Vec<(ActorRefStrong<PingerMsg>, ActorRef<Ping>)> =
            Vec::with_capacity(pairs);
        let mut pongers: Vec<Arc<Component<Ponger>>> = Vec::with_capacity(pairs);
        for _ in 0..pairs {
            let pingerc = system.create(|| Pinger::with(MSG_COUNT));
            let pinger_ref = pingerc.actor_ref().hold().expect("actor ref");
            let pongerc = system.create(|| Ponger::new(pinger_ref.weak_ref()));
            let ponger_ref = pongerc.actor_ref();
            start_refs.push((pinger_ref, ponger_ref));
            let pinger_f = system.start_notify(&pingerc);
            let ponger_f = system.start_notify(&pongerc);
            pinger_f
                .wait_timeout(timeout)
                .expect("Pinger didn't start.");
            ponger_f
                .wait_timeout(timeout)
                .expect("Pinger didn't start.");
            pingers.push(pingerc);
            pongers.push(pongerc);
        }
        b.iter_batched_ref(
            || Arc::new(CountdownEvent::new(pairs)),
            |latch| {
                for (pinger_ref, ponger_ref) in start_refs.iter() {
                    pinger_ref.tell(PingerMsg::Start {
                        latch: latch.clone(),
                        ponger: ponger_ref.clone(),
                    });
                }
                latch.wait();
            },
            BatchSize::PerIteration,
        );

        let mut futures = Vec::with_capacity(2 * pairs);
        drop(start_refs);
        for pinger in pingers {
            let f = system.kill_notify(pinger);
            futures.push(f);
        }
        for ponger in pongers {
            let f = system.kill_notify(ponger);
            futures.push(f);
        }
        for f in futures {
            f.wait_timeout(timeout)
                .expect("Some component didn't shut down properly!");
        }
        system
            .shutdown()
            .expect("Kompact didn't shut down properly");
    }
}

criterion_group!(pingperf_bench, ping_benches);
criterion_main!(pingperf_bench);

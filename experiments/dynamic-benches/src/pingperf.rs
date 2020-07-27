use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use kompact::prelude::*;
use std::{fmt, sync::Arc};
use synchronoise::CountdownEvent;

// #[inline(always)]
// pub fn ignore<V>(_: V) -> () {
//     ()
// }

const MSG_COUNT: u64 = 50000;
const LATENCY_MSG_COUNT: u64 = 10000;

pub fn ping_throughput_benches(c: &mut Criterion) {
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
        g.bench_with_input(
            BenchmarkId::new("ask", pairs),
            pairs,
            tests::ping_pong_throughput_ask,
        );
    }
    g.finish();
}

pub fn ping_latency_benches(c: &mut Criterion) {
    let mut g = c.benchmark_group("Ping Latency Benches");
    for pairs in [1usize].iter() {
        g.throughput(Throughput::Elements(
            2u64 * LATENCY_MSG_COUNT * (*pairs as u64),
        ));
        g.bench_with_input(
            BenchmarkId::new("ports", pairs),
            pairs,
            tests::ping_pong_latency_ports,
        );
        g.bench_with_input(
            BenchmarkId::new("strong-refs", pairs),
            pairs,
            tests::ping_pong_latency_strong_ref,
        );
        g.bench_with_input(
            BenchmarkId::new("weak-refs", pairs),
            pairs,
            tests::ping_pong_latency_weak_ref,
        );
        g.bench_with_input(
            BenchmarkId::new("ask", pairs),
            pairs,
            tests::ping_pong_latency_ask,
        );
    }
    g.finish();
}

const MAX_BATCH: u64 = 1000;

mod port_pingpong {
    use super::*;

    #[derive(Clone)]
    pub struct Start(pub Arc<CountdownEvent>);

    impl fmt::Debug for Start {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("Start(<CountdownEvent>)")
        }
    }

    #[derive(Clone, Debug)]
    pub struct Ping;
    #[derive(Clone, Debug)]
    pub struct Pong;

    pub struct PingPongPort;

    impl Port for PingPongPort {
        type Indication = Pong;
        type Request = Ping;
    }

    pub mod throughput {
        use super::*;

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
                    ctx: ComponentContext::uninitialised(),
                    ppp: RequiredPort::uninitialised(),
                    latch: None,
                    repeat,
                    sent: 0,
                    received: 0,
                }
            }
        }

        ignore_lifecycle!(Pinger);

        impl Require<PingPongPort> for Pinger {
            fn handle(&mut self, _event: Pong) -> Handled {
                self.received += 1;
                if self.sent < self.repeat {
                    self.ppp.trigger(Ping);
                    self.sent += 1;
                } else if self.received >= self.repeat {
                    let _ = self.latch.take().unwrap().decrement();
                }
                Handled::Ok
            }
        }

        impl Actor for Pinger {
            type Message = Start;

            fn receive_local(&mut self, msg: Self::Message) -> Handled {
                self.sent = 0;
                self.received = 0;
                self.latch = Some(msg.0);
                let remaining = std::cmp::min(MAX_BATCH, self.repeat);
                for _ in 0..remaining {
                    self.ppp.trigger(Ping);
                    self.sent += 1;
                }
                Handled::Ok
            }

            fn receive_network(&mut self, _msg: NetMessage) -> Handled {
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
                    ctx: ComponentContext::uninitialised(),
                    ppp: ProvidedPort::uninitialised(),
                    received: 0,
                }
            }
        }

        ignore_lifecycle!(Ponger);

        impl Provide<PingPongPort> for Ponger {
            fn handle(&mut self, _event: Ping) -> Handled {
                self.received += 1;
                //println!("Got Ping #{}", self.received);
                self.ppp.trigger(Pong);
                Handled::Ok
            }
        }
    }

    pub mod latency {
        use super::*;

        #[derive(ComponentDefinition)]
        pub struct Pinger {
            ctx: ComponentContext<Pinger>,
            ppp: RequiredPort<PingPongPort>,
            latch: Option<Arc<CountdownEvent>>,
            repeat: u64,
            received: u64,
        }

        impl Pinger {
            pub fn with(repeat: u64) -> Pinger {
                Pinger {
                    ctx: ComponentContext::uninitialised(),
                    ppp: RequiredPort::uninitialised(),
                    latch: None,
                    repeat,
                    received: 0,
                }
            }
        }

        ignore_lifecycle!(Pinger);

        impl Require<PingPongPort> for Pinger {
            fn handle(&mut self, _event: Pong) -> Handled {
                self.received += 1;
                if self.received < self.repeat {
                    self.ppp.trigger(Ping);
                } else {
                    let _ = self.latch.take().unwrap().decrement();
                }
                Handled::Ok
            }
        }

        impl Actor for Pinger {
            type Message = Start;

            fn receive_local(&mut self, msg: Self::Message) -> Handled {
                self.received = 0;
                self.latch = Some(msg.0);
                self.ppp.trigger(Ping);
                Handled::Ok
            }

            fn receive_network(&mut self, _msg: NetMessage) -> Handled {
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
                    ctx: ComponentContext::uninitialised(),
                    ppp: ProvidedPort::uninitialised(),
                    received: 0,
                }
            }
        }

        ignore_lifecycle!(Ponger);

        impl Provide<PingPongPort> for Ponger {
            fn handle(&mut self, _event: Ping) -> Handled {
                self.received += 1;
                //println!("Got Ping #{}", self.received);
                self.ppp.trigger(Pong);
                Handled::Ok
            }
        }
    }
}

mod strong_ref_pingpong {
    use super::*;

    #[derive(Clone, Debug)]
    pub struct Ping;
    #[derive(Clone, Debug)]
    pub struct Pong;

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

    pub mod throughput {
        use super::*;

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
                    ctx: ComponentContext::uninitialised(),
                    ponger: None,
                    latch: None,
                    repeat,
                    sent: 0,
                    received: 0,
                }
            }
        }

        ignore_lifecycle!(Pinger);

        impl Actor for Pinger {
            type Message = PingerMsg;

            fn receive_local(&mut self, msg: Self::Message) -> Handled {
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
                Handled::Ok
            }

            fn receive_network(&mut self, _msg: NetMessage) -> Handled {
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
                    ctx: ComponentContext::uninitialised(),
                    pinger,
                    received: 0,
                }
            }
        }

        ignore_lifecycle!(Ponger);

        impl Actor for Ponger {
            type Message = Ping;

            fn receive_local(&mut self, _msg: Self::Message) -> Handled {
                self.received += 1;
                self.pinger.tell(PingerMsg::Pong);
                Handled::Ok
            }

            fn receive_network(&mut self, _msg: NetMessage) -> Handled {
                unimplemented!("not needed");
            }
        }
    }

    pub mod latency {
        use super::*;

        #[derive(ComponentDefinition)]
        pub struct Pinger {
            ctx: ComponentContext<Pinger>,
            ponger: Option<ActorRefStrong<Ping>>,
            latch: Option<Arc<CountdownEvent>>,
            repeat: u64,
            received: u64,
        }

        impl Pinger {
            pub fn with(repeat: u64) -> Pinger {
                Pinger {
                    ctx: ComponentContext::uninitialised(),
                    ponger: None,
                    latch: None,
                    repeat,
                    received: 0,
                }
            }
        }

        ignore_lifecycle!(Pinger);

        impl Actor for Pinger {
            type Message = PingerMsg;

            fn receive_local(&mut self, msg: Self::Message) -> Handled {
                match msg {
                    PingerMsg::Start { ponger, latch } => {
                        self.received = 0;
                        self.latch = Some(latch);
                        ponger.tell(Ping);
                        self.ponger = Some(ponger);
                    }
                    PingerMsg::Pong => {
                        self.received += 1;
                        if self.received < self.repeat {
                            if let Some(ref ponger) = self.ponger {
                                ponger.tell(Ping);
                            } else {
                                unreachable!("Ponger should have been set here!");
                            }
                        } else {
                            let _ = self.latch.take().unwrap().decrement();
                        }
                    }
                }
                Handled::Ok
            }

            fn receive_network(&mut self, _msg: NetMessage) -> Handled {
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
                    ctx: ComponentContext::uninitialised(),
                    pinger,
                    received: 0,
                }
            }
        }

        ignore_lifecycle!(Ponger);

        impl Actor for Ponger {
            type Message = Ping;

            fn receive_local(&mut self, _msg: Self::Message) -> Handled {
                self.received += 1;
                self.pinger.tell(PingerMsg::Pong);
                Handled::Ok
            }

            fn receive_network(&mut self, _msg: NetMessage) -> Handled {
                unimplemented!("not needed");
            }
        }
    }
}

mod weak_ref_pingpong {
    use super::*;

    #[derive(Clone, Debug)]
    pub struct Ping;
    #[derive(Clone, Debug)]
    pub struct Pong;

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

    pub mod throughput {
        use super::*;

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
                    ctx: ComponentContext::uninitialised(),
                    ponger: None,
                    latch: None,
                    repeat,
                    sent: 0,
                    received: 0,
                }
            }
        }

        ignore_lifecycle!(Pinger);

        impl Actor for Pinger {
            type Message = PingerMsg;

            fn receive_local(&mut self, msg: Self::Message) -> Handled {
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
                Handled::Ok
            }

            fn receive_network(&mut self, _msg: NetMessage) -> Handled {
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
                    ctx: ComponentContext::uninitialised(),
                    pinger,
                    received: 0,
                }
            }
        }

        ignore_lifecycle!(Ponger);

        impl Actor for Ponger {
            type Message = Ping;

            fn receive_local(&mut self, _msg: Self::Message) -> Handled {
                self.received += 1;
                self.pinger.tell(PingerMsg::Pong);
                Handled::Ok
            }

            fn receive_network(&mut self, _msg: NetMessage) -> Handled {
                unimplemented!("not needed");
            }
        }
    }

    pub mod latency {
        use super::*;

        #[derive(ComponentDefinition)]
        pub struct Pinger {
            ctx: ComponentContext<Pinger>,
            ponger: Option<ActorRef<Ping>>,
            latch: Option<Arc<CountdownEvent>>,
            repeat: u64,
            received: u64,
        }

        impl Pinger {
            pub fn with(repeat: u64) -> Pinger {
                Pinger {
                    ctx: ComponentContext::uninitialised(),
                    ponger: None,
                    latch: None,
                    repeat,
                    received: 0,
                }
            }
        }

        ignore_lifecycle!(Pinger);

        impl Actor for Pinger {
            type Message = PingerMsg;

            fn receive_local(&mut self, msg: Self::Message) -> Handled {
                match msg {
                    PingerMsg::Start { ponger, latch } => {
                        self.received = 0;
                        self.latch = Some(latch);
                        ponger.tell(Ping);
                        self.ponger = Some(ponger);
                    }
                    PingerMsg::Pong => {
                        self.received += 1;
                        if self.received < self.repeat {
                            if let Some(ref ponger) = self.ponger {
                                ponger.tell(Ping);
                            } else {
                                unreachable!("Ponger should have been set here!");
                            }
                        } else {
                            let _ = self.latch.take().unwrap().decrement();
                        }
                    }
                }
                Handled::Ok
            }

            fn receive_network(&mut self, _msg: NetMessage) -> Handled {
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
                    ctx: ComponentContext::uninitialised(),
                    pinger,
                    received: 0,
                }
            }
        }

        ignore_lifecycle!(Ponger);

        impl Actor for Ponger {
            type Message = Ping;

            fn receive_local(&mut self, _msg: Self::Message) -> Handled {
                self.received += 1;
                self.pinger.tell(PingerMsg::Pong);
                Handled::Ok
            }

            fn receive_network(&mut self, _msg: NetMessage) -> Handled {
                unimplemented!("not needed");
            }
        }
    }
}

mod ask_pingpong {
    use super::*;

    pub struct PingerStart {
        pub latch: Arc<CountdownEvent>,
        pub ponger: ActorRefStrong<Ask<Ping, Pong>>,
    }

    #[derive(Debug, Clone)]
    pub struct Ping;
    #[derive(Debug, Clone)]
    pub struct Pong;

    impl fmt::Debug for PingerStart {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("PingerMsg::Start")
                .field("latch", &"<CountdownEvent>")
                .field("ponger", &"<ActorRef>")
                .finish()
        }
    }

    pub mod throughput {
        use super::*;

        #[derive(ComponentDefinition)]
        pub struct Pinger {
            ctx: ComponentContext<Self>,
            ponger: Option<ActorRefStrong<Ask<Ping, Pong>>>,
            latch: Option<Arc<CountdownEvent>>,
            repeat: u64,
            sent: u64,
            received: u64,
        }

        impl Pinger {
            pub fn with(repeat: u64) -> Pinger {
                Pinger {
                    ctx: ComponentContext::uninitialised(),
                    ponger: None,
                    latch: None,
                    repeat,
                    sent: 0,
                    received: 0,
                }
            }

            fn handle_pong(&mut self) -> Handled {
                //println!("Got Pong {}", self.received);
                self.received += 1;
                if self.sent < self.repeat {
                    if let Some(ref ponger) = self.ponger {
                        let f = ponger.ask(Ask::of(Ping));
                        self.sent += 1;
                        self.spawn_local(move |mut async_self| async move {
                            let _pong = f.await.expect("pong");
                            async_self.handle_pong()
                        });
                    } else {
                        unreachable!("Ponger should have been set here!");
                    }
                } else if self.received >= self.repeat {
                    //println!("Got last Pong");
                    let _ = self.latch.take().unwrap().decrement();
                }
                Handled::Ok
            }
        }

        ignore_lifecycle!(Pinger);

        impl Actor for Pinger {
            type Message = PingerStart;

            fn receive_local(&mut self, msg: Self::Message) -> Handled {
                let PingerStart { ponger, latch } = msg;
                self.sent = 0;
                self.received = 0;
                self.latch = Some(latch);
                let remaining = std::cmp::min(MAX_BATCH, self.repeat);
                for _ in 0..remaining {
                    //println!("Sending Ping {}", self.sent);
                    let f = ponger.ask(Ask::of(Ping));
                    self.sent += 1;
                    self.spawn_local(move |mut async_self| async move {
                        let _pong = f.await.expect("pong");
                        async_self.handle_pong()
                    });
                }
                self.ponger = Some(ponger);

                Handled::Ok
            }

            fn receive_network(&mut self, _msg: NetMessage) -> Handled {
                unimplemented!("not needed");
            }
        }

        #[derive(ComponentDefinition)]
        pub struct Ponger {
            ctx: ComponentContext<Ponger>,
            received: u64,
        }

        impl Ponger {
            pub fn new() -> Ponger {
                Ponger {
                    ctx: ComponentContext::uninitialised(),
                    received: 0,
                }
            }
        }

        ignore_lifecycle!(Ponger);

        impl Actor for Ponger {
            type Message = Ask<Ping, Pong>;

            fn receive_local(&mut self, msg: Self::Message) -> Handled {
                self.received += 1;
                msg.complete(|_| Pong).expect("should send pong");
                Handled::Ok
            }

            fn receive_network(&mut self, _msg: NetMessage) -> Handled {
                unimplemented!("not needed");
            }
        }
    }

    pub mod latency {
        use super::*;

        #[derive(ComponentDefinition)]
        pub struct Pinger {
            ctx: ComponentContext<Self>,
            repeat: u64,
            received: u64,
        }

        impl Pinger {
            pub fn with(repeat: u64) -> Pinger {
                Pinger {
                    ctx: ComponentContext::uninitialised(),
                    repeat,
                    received: 0,
                }
            }
        }

        ignore_lifecycle!(Pinger);

        impl Actor for Pinger {
            type Message = PingerStart;

            fn receive_local(&mut self, msg: Self::Message) -> Handled {
                let PingerStart { ponger, latch } = msg;
                self.received = 0;
                //println!("Sending Ping {}", self.sent);
                Handled::block_on(self, move |mut async_self| async move {
                    while async_self.received < async_self.repeat {
                        let _pong = ponger.ask(Ask::of(Ping)).await.expect("pong");
                        async_self.received += 1;
                    }
                    let _ = latch.decrement();
                })
            }

            fn receive_network(&mut self, _msg: NetMessage) -> Handled {
                unimplemented!("not needed");
            }
        }

        #[derive(ComponentDefinition)]
        pub struct Ponger {
            ctx: ComponentContext<Ponger>,
            received: u64,
        }

        impl Ponger {
            pub fn new() -> Ponger {
                Ponger {
                    ctx: ComponentContext::uninitialised(),
                    received: 0,
                }
            }
        }

        ignore_lifecycle!(Ponger);

        impl Actor for Ponger {
            type Message = Ask<Ping, Pong>;

            fn receive_local(&mut self, msg: Self::Message) -> Handled {
                self.received += 1;
                msg.complete(|_| Pong).expect("should send pong");
                Handled::Ok
            }

            fn receive_network(&mut self, _msg: NetMessage) -> Handled {
                unimplemented!("not needed");
            }
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
        use super::port_pingpong::{throughput::*, *};
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

    pub fn ping_pong_latency_ports(b: &mut Bencher, pairs: &usize) {
        use super::port_pingpong::{latency::*, *};
        let pairs = *pairs;
        let timeout = Duration::from_millis(1000);

        let system = setup_kompact_system();
        let mut pingers: Vec<Arc<Component<Pinger>>> = Vec::with_capacity(pairs);
        let mut start_refs: Vec<ActorRefStrong<Start>> = Vec::with_capacity(pairs);
        let mut pongers: Vec<Arc<Component<Ponger>>> = Vec::with_capacity(pairs);
        for _ in 0..pairs {
            let pingerc = system.create(|| Pinger::with(LATENCY_MSG_COUNT));
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
        use super::strong_ref_pingpong::{throughput::*, *};

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

    pub fn ping_pong_latency_strong_ref(b: &mut Bencher, pairs: &usize) {
        use super::strong_ref_pingpong::{latency::*, *};

        let pairs = *pairs;
        let timeout = Duration::from_millis(1000);

        let system = setup_kompact_system();
        let mut pingers: Vec<Arc<Component<Pinger>>> = Vec::with_capacity(pairs);
        let mut start_refs: Vec<(ActorRefStrong<PingerMsg>, ActorRefStrong<Ping>)> =
            Vec::with_capacity(pairs);
        let mut pongers: Vec<Arc<Component<Ponger>>> = Vec::with_capacity(pairs);
        for _ in 0..pairs {
            let pingerc = system.create(|| Pinger::with(LATENCY_MSG_COUNT));
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
        use super::weak_ref_pingpong::{throughput::*, *};

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

    pub fn ping_pong_latency_weak_ref(b: &mut Bencher, pairs: &usize) {
        use super::weak_ref_pingpong::{latency::*, *};

        let pairs = *pairs;
        let timeout = Duration::from_millis(1000);

        let system = setup_kompact_system();
        let mut pingers: Vec<Arc<Component<Pinger>>> = Vec::with_capacity(pairs);
        let mut start_refs: Vec<(ActorRefStrong<PingerMsg>, ActorRef<Ping>)> =
            Vec::with_capacity(pairs);
        let mut pongers: Vec<Arc<Component<Ponger>>> = Vec::with_capacity(pairs);
        for _ in 0..pairs {
            let pingerc = system.create(|| Pinger::with(LATENCY_MSG_COUNT));
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

    pub fn ping_pong_throughput_ask(b: &mut Bencher, pairs: &usize) {
        use super::ask_pingpong::{throughput::*, *};

        let pairs = *pairs;
        let timeout = Duration::from_millis(1000);

        let system = setup_kompact_system();
        let mut pingers: Vec<Arc<Component<Pinger>>> = Vec::with_capacity(pairs);
        let mut start_refs: Vec<(ActorRefStrong<PingerStart>, ActorRefStrong<Ask<Ping, Pong>>)> =
            Vec::with_capacity(pairs);
        let mut pongers: Vec<Arc<Component<Ponger>>> = Vec::with_capacity(pairs);
        for _ in 0..pairs {
            let pingerc = system.create(|| Pinger::with(MSG_COUNT));
            let pinger_ref = pingerc.actor_ref().hold().expect("actor ref");
            let pongerc = system.create(Ponger::new);
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
                    pinger_ref.tell(PingerStart {
                        latch: latch.clone(),
                        ponger: ponger_ref.clone(),
                    });
                }
                //println!("awaiting latch");
                latch.wait();
                //println!("got latch");
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

    pub fn ping_pong_latency_ask(b: &mut Bencher, pairs: &usize) {
        use super::ask_pingpong::{latency::*, *};

        let pairs = *pairs;
        let timeout = Duration::from_millis(1000);

        let system = setup_kompact_system();
        let mut pingers: Vec<Arc<Component<Pinger>>> = Vec::with_capacity(pairs);
        let mut start_refs: Vec<(ActorRefStrong<PingerStart>, ActorRefStrong<Ask<Ping, Pong>>)> =
            Vec::with_capacity(pairs);
        let mut pongers: Vec<Arc<Component<Ponger>>> = Vec::with_capacity(pairs);
        for _ in 0..pairs {
            let pingerc = system.create(|| Pinger::with(LATENCY_MSG_COUNT));
            let pinger_ref = pingerc.actor_ref().hold().expect("actor ref");
            let pongerc = system.create(Ponger::new);
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
                    pinger_ref.tell(PingerStart {
                        latch: latch.clone(),
                        ponger: ponger_ref.clone(),
                    });
                }
                //println!("awaiting latch");
                latch.wait();
                //println!("got latch");
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

criterion_group!(
    pingperf_bench,
    ping_throughput_benches,
    ping_latency_benches
);
criterion_main!(pingperf_bench);

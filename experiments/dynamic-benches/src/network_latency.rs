use criterion::{criterion_group, criterion_main, Bencher, BenchmarkId, Criterion, Throughput};
use std::time::{Duration, Instant};
//use kompact::*;
use kompact::prelude::*;
//use kompact::default_components::DeadletterBox;

const MSG_COUNT: u64 = 1000;

pub fn kompact_network_latency(c: &mut Criterion) {
    let mut g = c.benchmark_group("Ping Pong RTT");
    g.bench_function("Ping Pong RTT (Static)", ping_pong_latency_static);
    g.bench_function("Ping Pong RTT (Indexed)", ping_pong_latency_indexed);
    g.bench_function(
        "Ping Pong RTT Pipeline All (Static)",
        ping_pong_latency_pipeline_static,
    );
    g.bench_function(
        "Ping Pong RTT Pipeline All (Indexed)",
        ping_pong_latency_pipeline_indexed,
    );
    g.finish();

    let mut group = c.benchmark_group("Ping Pong RTT (Static) by Threadpool Size");
    for threads in 1..8 {
        group.bench_with_input(
            BenchmarkId::from_parameter(threads),
            &threads,
            ping_pong_latency_static_threads,
        );
    }
    group.finish();
}

pub fn kompact_network_throughput(c: &mut Criterion) {
    let mut g = c.benchmark_group("Ping Pong Throughput with Pipelining");
    g.throughput(Throughput::Elements(2 * MSG_COUNT));
    for pipeline in [1u64, 10u64, 100u64, 1000u64].iter() {
        //...[1u64, ...
        g.bench_with_input(
            BenchmarkId::from_parameter(pipeline),
            pipeline,
            ping_pong_throughput_static,
        );
    }
    g.finish();
}

pub fn latch_overhead(c: &mut Criterion) {
    c.bench_function("Synchronoise Latch", latch_synchronoise);
}

pub fn latch_synchronoise(b: &mut Bencher) {
    use std::sync::Arc;
    use synchronoise::CountdownEvent;

    b.iter_custom(|num_iterations| {
        let mut outer_latches: Vec<Arc<CountdownEvent>> = vec![(); num_iterations as usize]
            .drain(..)
            .map(|_| Arc::new(CountdownEvent::new(1)))
            .collect();
        let mut inner_latches: Vec<Arc<CountdownEvent>> = vec![(); num_iterations as usize]
            .drain(..)
            .map(|_| Arc::new(CountdownEvent::new(1)))
            .collect();
        let mut outer_latches_thread = outer_latches.clone();
        let mut inner_latches_thread = inner_latches.clone();
        std::thread::spawn(move || {
            loop {
                if let Some(wait_latch) = outer_latches_thread.pop() {
                    wait_latch.wait();
                    let write_latch = inner_latches_thread.pop().unwrap(); // both vectors are same size
                    write_latch.decrement().expect("Should decrement!");
                } else {
                    return;
                }
            }
        });
        let start = Instant::now();
        while let Some(write_latch) = outer_latches.pop() {
            write_latch.decrement().expect("Should decrement!");
            let wait_latch = inner_latches.pop().unwrap();
            wait_latch.wait();
        }
        let diff = start.elapsed();
        diff
    });
}

pub fn as_binary() {
    use ppstatic::*;
    ping_pong_latency_bin(
        100000,
        |ponger| Pinger::new(ponger),
        Ponger::new,
        |pinger| pinger.on_definition(|cd| cd.experiment_port()),
    );
}

fn ping_pong_latency_bin<Pinger, PingerF, Ponger, PongerF, PortF>(
    iterations: u64,
    pinger_func: PingerF,
    ponger_func: PongerF,
    port_func: PortF,
) -> Duration
    where
        Pinger: ComponentDefinition + 'static,
        Ponger: ComponentDefinition + 'static,
        PingerF: Fn(ActorPath) -> Pinger,
        PongerF: Fn() -> Ponger,
        PortF: Fn(&std::sync::Arc<Component<Pinger>>) -> ProvidedRef<ExperimentPort>,
{
    // Setup
    let sys1 = setup_system("test-system-1", 1);
    let sys2 = setup_system("test-system-2", 1);

    let timeout = Duration::from_millis(500);

    let (ponger, ponger_f) = sys2.create_and_register(ponger_func);
    let ponger_path = ponger_f.wait_expect(timeout, "Ponger failed to register!");

    let (pinger, pinger_f) = sys1.create_and_register(move || pinger_func(ponger_path));
    let _pinger_path = pinger_f.wait_expect(timeout, "Pinger failed to register!");

    let experiment_port = port_func(&pinger);

    sys2.start_notify(&ponger)
        .wait_timeout(timeout)
        .expect("Ponger never started!");
    sys1.start_notify(&pinger)
        .wait_timeout(timeout)
        .expect("Pinger never started!");

    let (promise, future) = kpromise();
    sys1.trigger_r(Run::new(iterations, promise), &experiment_port);
    let res = future.wait();

    // Teardown
    drop(experiment_port);
    drop(pinger);
    drop(ponger);
    sys1.shutdown().expect("System 1 did not shut down!");
    sys2.shutdown().expect("System 2 did not shut down!");
    res
}

fn setup_system(name: &'static str, threads: usize) -> KompactSystem {
    let mut cfg = KompactConfig::new();
    cfg.label(name.to_string());
    cfg.threads(threads);
    cfg.system_components(DeadletterBox::new, {
        let net_config = NetworkConfig::new("127.0.0.1:0".parse().expect("Address should work"));
        net_config.build()
    });
    cfg.build().expect("KompactSystem")
}

pub fn ping_pong_throughput_static(b: &mut Bencher, pipeline: &u64) {
    use ppstatic::pipelined::*;
    ping_pong_latency(
        b,
        4,
        |ponger| Pinger::with(MSG_COUNT, *pipeline, ponger),
        Ponger::new,
        |pinger| pinger.on_definition(|cd| cd.experiment_port()),
    );
}

pub fn ping_pong_latency_static(b: &mut Bencher) {
    use ppstatic::*;
    ping_pong_latency(
        b,
        1,
        |ponger| Pinger::new(ponger),
        Ponger::new,
        |pinger| pinger.on_definition(|cd| cd.experiment_port()),
    );
}

pub fn ping_pong_latency_static_threads(b: &mut Bencher, threads: &usize) {
    use ppstatic::*;
    ping_pong_latency(
        b,
        *threads,
        |ponger| Pinger::new(ponger),
        Ponger::new,
        |pinger| pinger.on_definition(|cd| cd.experiment_port()),
    );
}

pub fn ping_pong_latency_indexed(b: &mut Bencher) {
    use ppindexed::*;
    ping_pong_latency(
        b,
        1,
        |ponger| Pinger::new(ponger),
        Ponger::new,
        |pinger| pinger.on_definition(|cd| cd.experiment_port()),
    );
}

pub fn ping_pong_latency_pipeline_static(b: &mut Bencher) {
    use pppipelinestatic::*;
    ping_pong_latency(
        b,
        1,
        |ponger| Pinger::new(ponger),
        Ponger::new,
        |pinger| pinger.on_definition(|cd| cd.experiment_port()),
    );
}

pub fn ping_pong_latency_pipeline_indexed(b: &mut Bencher) {
    use pppipelineindexed::*;
    ping_pong_latency(
        b,
        1,
        |ponger| Pinger::new(ponger),
        Ponger::new,
        |pinger| pinger.on_definition(|cd| cd.experiment_port()),
    );
}

fn ping_pong_latency<Pinger, PingerF, Ponger, PongerF, PortF>(
    b: &mut Bencher,
    threads: usize,
    pinger_func: PingerF,
    ponger_func: PongerF,
    port_func: PortF,
) where
    Pinger: ComponentDefinition + 'static,
    Ponger: ComponentDefinition + 'static,
    PingerF: Fn(ActorPath) -> Pinger,
    PongerF: Fn() -> Ponger,
    PortF: Fn(&std::sync::Arc<Component<Pinger>>) -> ProvidedRef<ExperimentPort>,
{
    // Setup
    let sys1 = setup_system("test-system-1", threads);
    let sys2 = setup_system("test-system-2", threads);

    let timeout = Duration::from_millis(500);

    let (ponger, ponger_f) = sys2.create_and_register(ponger_func);
    let ponger_path = ponger_f.wait_expect(timeout, "Ponger failed to register!");

    let (pinger, pinger_f) = sys1.create_and_register(move || pinger_func(ponger_path));
    let _pinger_path = pinger_f.wait_expect(timeout, "Pinger failed to register!");

    let experiment_port = port_func(&pinger);

    sys2.start_notify(&ponger)
        .wait_timeout(timeout)
        .expect("Ponger never started!");
    sys1.start_notify(&pinger)
        .wait_timeout(timeout)
        .expect("Pinger never started!");

    b.iter_custom(|num_iterations| {
        let (promise, future) = kpromise();
        sys1.trigger_r(Run::new(num_iterations, promise), &experiment_port);
        let res = future.wait();
        res
    });

    // Teardown
    drop(experiment_port);
    drop(pinger);
    drop(ponger);
    sys1.shutdown().expect("System 1 did not shut down!");
    sys2.shutdown().expect("System 2 did not shut down!");
}

criterion_group!(
    latency_benches,
    kompact_network_latency,
    latch_overhead,
    kompact_network_throughput
);
criterion_main!(latency_benches);

#[derive(Debug)]
pub struct Run {
    num_iterations: u64,
    promise: KPromise<Duration>,
}

impl Run {
    pub fn new(num_iterations: u64, promise: KPromise<Duration>) -> Run {
        Run {
            num_iterations,
            promise,
        }
    }
}

impl Clone for Run {
    fn clone(&self) -> Self {
        unimplemented!("Shouldn't be invoked in this experiment!");
    }
}

pub struct ExperimentPort;

impl Port for ExperimentPort {
    type Indication = ();
    type Request = Run;
}

pub mod pppipelinestatic {
    use super::*;

    #[derive(ComponentDefinition)]
    pub struct Pinger {
        ctx: ComponentContext<Self>,
        experiment_port: ProvidedPort<ExperimentPort, Self>,
        ponger: ActorPath,
        remaining_send: u64,
        remaining_recv: u64,
        done: Option<KPromise<Duration>>,
        start: Instant,
    }

    impl Pinger {
        pub fn new(ponger: ActorPath) -> Pinger {
            Pinger {
                ctx: ComponentContext::new(),
                experiment_port: ProvidedPort::new(),
                ponger,
                remaining_send: 0u64,
                remaining_recv: 0u64,
                done: None,
                start: Instant::now(),
            }
        }

        pub fn experiment_port(&mut self) -> ProvidedRef<ExperimentPort> {
            self.experiment_port.share()
        }
    }

    impl Provide<ControlPort> for Pinger {
        fn handle(&mut self, event: ControlEvent) -> () {
            match event {
                ControlEvent::Start => {
                    debug!(self.ctx.log(), "Starting Pinger");
                }
                e => {
                    debug!(self.ctx.log(), "Pinger got control event: {:?}", e);
                }
            }
        }
    }

    impl Provide<ExperimentPort> for Pinger {
        fn handle(&mut self, event: Run) -> () {
            trace!(
                self.ctx.log(),
                "Pinger starting run with {} iterations !",
                event.num_iterations
            );
            self.remaining_send = event.num_iterations;
            self.remaining_recv = event.num_iterations;
            self.done = Some(event.promise);
            self.start = Instant::now();
            while self.remaining_send > 0u64 {
                self.remaining_send -= 1u64;
                self.ponger.tell_serialised(Ping::EVENT, self)
                    .expect("serialise");
            }
        }
    }

    impl Actor for Pinger {
        type Message = Never;

        fn receive_local(&mut self, _msg: Self::Message) -> () {
            unimplemented!("Never can't be instantiated!");
        }

        fn receive_network(&mut self, msg: NetMessage) -> () {
            trace!(self.ctx.log(), "Pinger received msg {:?}", msg, );
            match msg.try_deserialise::<Pong, Pong>() {
                Ok(_pong) => {
                    self.remaining_recv -= 1u64;
                    trace!(self.ctx.log(), "Pinger got Pong #{}!", self.remaining_recv);
                    if self.remaining_recv <= 0u64 {
                        let time = self.start.elapsed();
                        trace!(self.ctx.log(), "Pinger is done! Run took {:?}", time);
                        let promise = self.done.take().expect("No promise to reply to?");
                        promise.fulfill(time).expect("Promise was dropped");
                    }
                }
                Err(e) => {
                    error!(self.ctx.log(), "Error deserialising Pong: {:?}", e);
                    unimplemented!("Pinger received unexpected message!")
                }
            }
        }
    }

    #[derive(ComponentDefinition)]
    pub struct Ponger {
        ctx: ComponentContext<Self>,
    }

    impl Ponger {
        pub fn new() -> Ponger {
            Ponger {
                ctx: ComponentContext::new(),
            }
        }
    }

    impl Provide<ControlPort> for Ponger {
        fn handle(&mut self, event: ControlEvent) -> () {
            match event {
                ControlEvent::Start => {
                    debug!(self.ctx.log(), "Starting Ponger");
                }
                e => {
                    debug!(self.ctx.log(), "Ponger got control event: {:?}", e);
                }
            }
        }
    }

    impl Actor for Ponger {
        type Message = Never;

        fn receive_local(&mut self, _msg: Self::Message) -> () {
            unimplemented!("Never can't be instantiated!");
        }

        fn receive_network(&mut self, msg: NetMessage) -> () {
            trace!(self.ctx.log(), "Ponger received msg {:?}", msg, );
            let sender = msg.sender;
            match msg.data.try_deserialise::<Ping, Ping>() {
                Ok(_ping) => {
                    trace!(self.ctx.log(), "Ponger got Ping!");
                    sender.tell_serialised(Pong::EVENT, self)
                        .expect("serialise");
                }
                Err(e) => {
                    error!(self.ctx.log(), "Error deserialising Ping: {:?}", e);
                    unimplemented!("Ponger received unexpected message!");
                }
            }
        }
    }

    #[derive(Debug, Clone, Copy)]
    pub struct Ping;

    impl Ping {
        pub const EVENT: Ping = Ping {};
        pub const SERID: SerId = 42;
    }

    impl Serialisable for Ping {
        #[inline(always)]
        fn ser_id(&self) -> SerId {
            Ping::SERID
        }

        fn size_hint(&self) -> Option<usize> {
            Some(0)
        }

        fn serialise(&self, _buf: &mut dyn BufMut) -> Result<(), SerError> {
            Ok(())
        }

        fn local(self: Box<Self>) -> Result<Box<dyn Any + Send>, Box<dyn Serialisable>> {
            Ok(self)
        }
    }

    impl Deserialiser<Ping> for Ping {
        const SER_ID: SerId = Ping::SERID;

        fn deserialise(_buf: &mut dyn Buf) -> Result<Ping, SerError> {
            Ok(Ping)
        }
    }

    #[derive(Debug, Clone, Copy)]
    pub struct Pong;

    impl Pong {
        pub const EVENT: Pong = Pong {};
        pub const SERID: SerId = 43;
    }

    impl Serialisable for Pong {
        #[inline(always)]
        fn ser_id(&self) -> SerId {
            Pong::SERID
        }

        fn size_hint(&self) -> Option<usize> {
            Some(0)
        }

        fn serialise(&self, _buf: &mut dyn BufMut) -> Result<(), SerError> {
            Ok(())
        }

        fn local(self: Box<Self>) -> Result<Box<dyn Any + Send>, Box<dyn Serialisable>> {
            Ok(self)
        }
    }

    impl Deserialiser<Pong> for Pong {
        const SER_ID: SerId = Pong::SERID;

        fn deserialise(_buf: &mut dyn Buf) -> Result<Pong, SerError> {
            Ok(Pong)
        }
    }
}

pub mod pppipelineindexed {
    use super::*;

    #[derive(ComponentDefinition)]
    pub struct Pinger {
        ctx: ComponentContext<Self>,
        experiment_port: ProvidedPort<ExperimentPort, Self>,
        ponger: ActorPath,
        remaining_send: u64,
        remaining_recv: u64,
        done: Option<KPromise<Duration>>,
        start: Instant,
    }

    impl Pinger {
        pub fn new(ponger: ActorPath) -> Pinger {
            Pinger {
                ctx: ComponentContext::new(),
                experiment_port: ProvidedPort::new(),
                ponger,
                remaining_send: 0u64,
                remaining_recv: 0u64,
                done: None,
                start: Instant::now(),
            }
        }

        pub fn experiment_port(&mut self) -> ProvidedRef<ExperimentPort> {
            self.experiment_port.share()
        }
    }

    impl Provide<ControlPort> for Pinger {
        fn handle(&mut self, event: ControlEvent) -> () {
            match event {
                ControlEvent::Start => {
                    debug!(self.ctx.log(), "Starting Pinger");
                }
                e => {
                    debug!(self.ctx.log(), "Pinger got control event: {:?}", e);
                }
            }
        }
    }

    impl Provide<ExperimentPort> for Pinger {
        fn handle(&mut self, event: Run) -> () {
            trace!(
                self.ctx.log(),
                "Pinger starting run with {} iterations !",
                event.num_iterations
            );
            self.remaining_send = event.num_iterations;
            self.remaining_recv = event.num_iterations;
            self.done = Some(event.promise);
            self.start = Instant::now();

            while self.remaining_send > 0u64 {
                self.remaining_send -= 1u64;
                self.ponger
                    .tell_serialised(Ping::new(self.remaining_send), self)
                    .expect("serialise");
            }
        }
    }

    impl Actor for Pinger {
        type Message = Never;

        fn receive_local(&mut self, _msg: Self::Message) -> () {
            unimplemented!("Can't instantiate Never!");
        }

        fn receive_network(&mut self, msg: NetMessage) -> () {
            trace!(self.ctx.log(), "Pinger received msg {:?}", msg, );
            match msg.data.try_deserialise::<Pong, Pong>() {
                Ok(_pong) => {
                    self.remaining_recv -= 1u64;
                    trace!(self.ctx.log(), "Pinger got Pong #{}!", self.remaining_recv);
                    if self.remaining_recv <= 0u64 {
                        let time = self.start.elapsed();
                        trace!(self.ctx.log(), "Pinger is done! Run took {:?}", time);
                        let promise = self.done.take().expect("No promise to reply to?");
                        promise.fulfill(time).expect("Promise was dropped");
                    }
                }
                Err(e) => {
                    error!(self.ctx.log(), "Error deserialising Ping: {:?}", e);
                    unimplemented!("Pinger received unexpected message!");
                }
            }
        }
    }

    #[derive(ComponentDefinition)]
    pub struct Ponger {
        ctx: ComponentContext<Self>,
    }

    impl Ponger {
        pub fn new() -> Ponger {
            Ponger {
                ctx: ComponentContext::new(),
            }
        }
    }

    impl Provide<ControlPort> for Ponger {
        fn handle(&mut self, event: ControlEvent) -> () {
            match event {
                ControlEvent::Start => {
                    debug!(self.ctx.log(), "Starting Ponger");
                }
                e => {
                    debug!(self.ctx.log(), "Ponger got control event: {:?}", e);
                }
            }
        }
    }

    impl Actor for Ponger {
        type Message = Never;

        fn receive_local(&mut self, _msg: Self::Message) -> () {
            unimplemented!("Never can't be instantiated!");
        }

        fn receive_network(&mut self, msg: NetMessage) -> () {
            trace!(self.ctx.log(), "Ponger received msg {:?}", msg, );
            let sender = msg.sender;
            match msg.data.try_deserialise::<Ping, Ping>() {
                Ok(ping) => {
                    trace!(self.ctx.log(), "Ponger got Ping!");
                    sender.tell_serialised(Pong::new(ping.index), self)
                        .expect("serialise");
                }
                Err(e) => {
                    error!(self.ctx.log(), "Error deserialising Ping: {:?}", e);
                    unimplemented!("Ponger received unexpected message!");
                }
            }
        }
    }

    #[derive(Debug, Clone, Copy)]
    pub struct Ping {
        index: u64,
    }

    impl Ping {
        pub const SERID: SerId = 42;

        pub fn new(index: u64) -> Ping {
            Ping { index }
        }
    }

    impl Serialisable for Ping {
        #[inline(always)]
        fn ser_id(&self) -> SerId {
            Ping::SERID
        }

        fn size_hint(&self) -> Option<usize> {
            Some(8)
        }

        fn serialise(&self, buf: &mut dyn BufMut) -> Result<(), SerError> {
            buf.put_u64(self.index);
            Ok(())
        }

        fn local(self: Box<Self>) -> Result<Box<dyn Any + Send>, Box<dyn Serialisable>> {
            Ok(self)
        }
    }

    impl Deserialiser<Ping> for Ping {
        const SER_ID: SerId = Ping::SERID;

        fn deserialise(buf: &mut dyn Buf) -> Result<Ping, SerError> {
            let index = buf.get_u64();
            Ok(Ping::new(index))
        }
    }

    #[derive(Debug, Clone, Copy)]
    pub struct Pong {
        index: u64,
    }

    impl Pong {
        pub const SERID: SerId = 43;

        pub fn new(index: u64) -> Pong {
            Pong { index }
        }
    }

    impl Serialisable for Pong {
        #[inline(always)]
        fn ser_id(&self) -> SerId {
            Pong::SERID
        }

        fn size_hint(&self) -> Option<usize> {
            Some(8)
        }

        fn serialise(&self, buf: &mut dyn BufMut) -> Result<(), SerError> {
            buf.put_u64(self.index);
            Ok(())
        }

        fn local(self: Box<Self>) -> Result<Box<dyn Any + Send>, Box<dyn Serialisable>> {
            Ok(self)
        }
    }

    impl Deserialiser<Pong> for Pong {
        const SER_ID: SerId = Pong::SERID;

        fn deserialise(buf: &mut dyn Buf) -> Result<Pong, SerError> {
            let index = buf.get_u64();
            Ok(Pong::new(index))
        }
    }
}

pub mod ppstatic {
    use super::*;

    #[derive(ComponentDefinition)]
    pub struct Pinger {
        ctx: ComponentContext<Self>,
        experiment_port: ProvidedPort<ExperimentPort, Self>,
        ponger: ActorPath,
        remaining: u64,
        done: Option<KPromise<Duration>>,
        start: Instant,
    }

    impl Pinger {
        pub fn new(ponger: ActorPath) -> Pinger {
            Pinger {
                ctx: ComponentContext::new(),
                experiment_port: ProvidedPort::new(),
                ponger,
                remaining: 0u64,
                done: None,
                start: Instant::now(),
            }
        }

        pub fn experiment_port(&mut self) -> ProvidedRef<ExperimentPort> {
            self.experiment_port.share()
        }
    }

    impl Provide<ControlPort> for Pinger {
        fn handle(&mut self, event: ControlEvent) -> () {
            match event {
                ControlEvent::Start => {
                    debug!(self.ctx.log(), "Starting Pinger");
                }
                e => {
                    debug!(self.ctx.log(), "Pinger got control event: {:?}", e);
                }
            }
        }
    }

    impl Provide<ExperimentPort> for Pinger {
        fn handle(&mut self, event: Run) -> () {
            trace!(
                self.ctx.log(),
                "Pinger starting run with {} iterations !",
                event.num_iterations
            );
            self.remaining = event.num_iterations;
            self.done = Some(event.promise);
            self.start = Instant::now();
            self.ponger.tell_serialised(Ping::EVENT, self)
                .expect("serialise");
        }
    }

    impl Actor for Pinger {
        type Message = Never;

        fn receive_local(&mut self, _msg: Self::Message) -> () {
            unimplemented!("Never can't be instantiated!");
        }

        fn receive_network(&mut self, msg: NetMessage) -> () {
            trace!(self.ctx.log(), "Pinger received msg {:?}", msg, );
            let sender = msg.sender;
            match msg.data.try_deserialise::<Pong, Pong>() {
                Ok(_pong) => {
                    self.remaining -= 1u64;
                    trace!(self.ctx.log(), "Pinger got Pong #{}!", self.remaining);
                    if self.remaining > 0u64 {
                        sender.tell_serialised(Ping::EVENT, self)
                            .expect("serialise");
                    } else {
                        let time = self.start.elapsed();
                        trace!(self.ctx.log(), "Pinger is done! Run took {:?}", time);
                        let promise = self.done.take().expect("No promise to reply to?");
                        promise.fulfill(time).expect("Promise was dropped");
                    }
                }
                Err(e) => {
                    error!(self.ctx.log(), "Error deserialising Ping: {:?}", e);
                    unimplemented!("Pinger received unexpected message!");
                }
            }
        }
    }

    #[derive(ComponentDefinition)]
    pub struct Ponger {
        ctx: ComponentContext<Self>,
    }

    impl Ponger {
        pub fn new() -> Ponger {
            Ponger {
                ctx: ComponentContext::new(),
            }
        }
    }

    impl Provide<ControlPort> for Ponger {
        fn handle(&mut self, event: ControlEvent) -> () {
            match event {
                ControlEvent::Start => {
                    debug!(self.ctx.log(), "Starting Ponger");
                }
                e => {
                    debug!(self.ctx.log(), "Ponger got control event: {:?}", e);
                }
            }
        }
    }

    impl Actor for Ponger {
        type Message = Never;

        fn receive_local(&mut self, _msg: Self::Message) -> () {
            unimplemented!("Never can't be instantiated!");
        }

        fn receive_network(&mut self, msg: NetMessage) -> () {
            trace!(self.ctx.log(), "Ponger received msg {:?}", msg, );
            let sender = msg.sender;
            match msg.data.try_deserialise::<Ping, Ping>() {
                Ok(_ping) => {
                    trace!(self.ctx.log(), "Ponger got Ping!");
                    sender.tell_serialised(Pong::EVENT, self)
                        .expect("serialise");
                }
                Err(e) => {
                    error!(self.ctx.log(), "Error deserialising Ping: {:?}", e);
                    unimplemented!("Ponger received unexpected message!");
                }
            }
        }
    }

    #[derive(Debug, Clone, Copy)]
    pub struct Ping;

    impl Ping {
        pub const EVENT: Ping = Ping {};
        pub const SERID: SerId = 42;
    }

    impl Serialisable for Ping {
        #[inline(always)]
        fn ser_id(&self) -> SerId {
            Ping::SERID
        }

        fn size_hint(&self) -> Option<usize> {
            Some(0)
        }

        fn serialise(&self, _buf: &mut dyn BufMut) -> Result<(), SerError> {
            Ok(())
        }

        fn local(self: Box<Self>) -> Result<Box<dyn Any + Send>, Box<dyn Serialisable>> {
            Ok(self)
        }
    }

    impl Deserialiser<Ping> for Ping {
        const SER_ID: SerId = Ping::SERID;

        fn deserialise(_buf: &mut dyn Buf) -> Result<Ping, SerError> {
            Ok(Ping)
        }
    }

    #[derive(Debug, Clone, Copy)]
    pub struct Pong;

    impl Pong {
        pub const EVENT: Pong = Pong {};
        pub const SERID: SerId = 43;
    }

    impl Serialisable for Pong {
        #[inline(always)]
        fn ser_id(&self) -> SerId {
            Pong::SERID
        }

        fn size_hint(&self) -> Option<usize> {
            Some(0)
        }

        fn serialise(&self, _buf: &mut dyn BufMut) -> Result<(), SerError> {
            Ok(())
        }

        fn local(self: Box<Self>) -> Result<Box<dyn Any + Send>, Box<dyn Serialisable>> {
            Ok(self)
        }
    }

    impl Deserialiser<Pong> for Pong {
        const SER_ID: SerId = Pong::SERID;

        fn deserialise(_buf: &mut dyn Buf) -> Result<Pong, SerError> {
            Ok(Pong)
        }
    }

    pub mod pipelined {
        use super::{ExperimentPort, Ping, Pong, Run};
        use kompact::prelude::*;
        use std::time::{Duration, Instant};

        #[derive(ComponentDefinition)]
        pub struct Pinger {
            ctx: ComponentContext<Self>,
            experiment_port: ProvidedPort<ExperimentPort, Self>,
            done: Option<KPromise<Duration>>,
            ponger: ActorPath,
            count: u64,
            pipeline: u64,
            sent_count: u64,
            recv_count: u64,
            iters: u64,
            start: Instant,
        }

        impl Pinger {
            pub fn with(count: u64, pipeline: u64, ponger: ActorPath) -> Pinger {
                Pinger {
                    ctx: ComponentContext::new(),
                    experiment_port: ProvidedPort::new(),
                    done: None,
                    ponger,
                    count,
                    pipeline,
                    sent_count: 0u64,
                    recv_count: 0u64,
                    iters: 0u64,
                    start: Instant::now(),
                }
            }

            pub fn experiment_port(&mut self) -> ProvidedRef<ExperimentPort> {
                self.experiment_port.share()
            }
        }

        impl Provide<ControlPort> for Pinger {
            fn handle(&mut self, _event: ControlEvent) -> () {
                // ignore
            }
        }

        impl Provide<ExperimentPort> for Pinger {
            fn handle(&mut self, event: Run) -> () {
                trace!(
                    self.ctx.log(),
                    "Pinger starting run with {} iterations !",
                    event.num_iterations
                );
                self.iters = event.num_iterations;
                self.done = Some(event.promise);
                self.start = Instant::now();
                let mut pipelined: u64 = 0;
                while (pipelined < self.pipeline) && (self.sent_count < self.count) {
                    self.ponger.tell_serialised(Ping::EVENT, self)
                        .expect("serialise");
                    self.sent_count += 1;
                    pipelined += 1;
                }
            }
        }

        impl Actor for Pinger {
            type Message = Never;

            fn receive_local(&mut self, _msg: Self::Message) -> () {
                unimplemented!("Never can't be instantiated!");
            }

            fn receive_network(&mut self, msg: NetMessage) -> () {
                trace!(self.ctx.log(), "Pinger received msg {:?}", msg);
                match msg.data.try_deserialise::<Pong, Pong>() {
                    Ok(_pong) => {
                        self.recv_count += 1;
                        if self.recv_count < self.count {
                            if self.sent_count < self.count {
                                self.ponger.tell_serialised(Ping::EVENT, self)
                                    .expect("serialise");
                                self.sent_count += 1;
                            }
                        } else {
                            self.iters -= 1;
                            if self.iters > 0u64 {
                                self.sent_count = 0u64;
                                self.recv_count = 0u64;
                                let mut pipelined: u64 = 0;
                                while (pipelined < self.pipeline) && (self.sent_count < self.count)
                                {
                                    self.ponger.tell_serialised(Ping::EVENT, self)
                                        .expect("serialise");
                                    self.sent_count += 1;
                                    pipelined += 1;
                                }
                            } else {
                                let time = self.start.elapsed();
                                self.done
                                    .take()
                                    .expect("No promise?")
                                    .fulfill(time)
                                    .expect("Why no fulfillment?");
                            }
                        }
                    }
                    Err(e) => {
                        error!(self.ctx.log(), "Error deserialising Pong: {:?}", e);
                        unimplemented!("Pinger received unexpected message!")
                    }
                }
            }
        }

        /*****************
         * Static Ponger *
         *****************/

        #[derive(ComponentDefinition)]
        pub struct Ponger {
            ctx: ComponentContext<Self>,
        }

        impl Ponger {
            pub fn new() -> Ponger {
                Ponger {
                    ctx: ComponentContext::new(),
                }
            }
        }

        impl Provide<ControlPort> for Ponger {
            fn handle(&mut self, _event: ControlEvent) -> () {
                // ignore
            }
        }

        impl Actor for Ponger {
            type Message = Never;

            fn receive_local(&mut self, _msg: Self::Message) -> () {
                unimplemented!("Never can't be instantiated!");
            }

            fn receive_network(&mut self, msg: NetMessage) -> () {
                trace!(self.ctx.log(), "Ponger received msg {:?}", msg, );
                let sender = msg.sender;
                match msg.data.try_deserialise::<Ping, Ping>() {
                    Ok(_ping) => {
                        trace!(self.ctx.log(), "Ponger got Ping!");
                        sender.tell_serialised(Pong::EVENT, self)
                            .expect("serialise");
                    }
                    Err(e) => {
                        error!(self.ctx.log(), "Error deserialising Ping: {:?}", e);
                        unimplemented!("Ponger received unexpected message!");
                    }
                }
            }
        }
    }
}

pub mod ppindexed {
    use super::*;

    #[derive(ComponentDefinition)]
    pub struct Pinger {
        ctx: ComponentContext<Self>,
        experiment_port: ProvidedPort<ExperimentPort, Self>,
        ponger: ActorPath,
        remaining: u64,
        done: Option<KPromise<Duration>>,
        start: Instant,
    }

    impl Pinger {
        pub fn new(ponger: ActorPath) -> Pinger {
            Pinger {
                ctx: ComponentContext::new(),
                experiment_port: ProvidedPort::new(),
                ponger,
                remaining: 0u64,
                done: None,
                start: Instant::now(),
            }
        }

        pub fn experiment_port(&mut self) -> ProvidedRef<ExperimentPort> {
            self.experiment_port.share()
        }
    }

    impl Provide<ControlPort> for Pinger {
        fn handle(&mut self, event: ControlEvent) -> () {
            match event {
                ControlEvent::Start => {
                    debug!(self.ctx.log(), "Starting Pinger");
                }
                e => {
                    debug!(self.ctx.log(), "Pinger got control event: {:?}", e);
                }
            }
        }
    }

    impl Provide<ExperimentPort> for Pinger {
        fn handle(&mut self, event: Run) -> () {
            trace!(
                self.ctx.log(),
                "Pinger starting run with {} iterations !",
                event.num_iterations
            );
            self.remaining = event.num_iterations;
            self.done = Some(event.promise);
            self.start = Instant::now();
            self.ponger.tell_serialised(Ping::new(self.remaining), self)
                .expect("serialise");
        }
    }

    impl Actor for Pinger {
        type Message = Never;

        fn receive_local(&mut self, _msg: Self::Message) -> () {
            unimplemented!("Never can't be instantiated!");
        }

        fn receive_network(&mut self, msg: NetMessage) -> () {
            trace!(self.ctx.log(), "Pinger received msg {:?}", msg, );
            match msg.data.try_deserialise::<Pong, Pong>() {
                Ok(_pong) => {
                    self.remaining -= 1u64;
                    trace!(self.ctx.log(), "Pinger got Pong #{}!", self.remaining);
                    if self.remaining > 0u64 {
                        self.ponger.tell_serialised(Ping::new(self.remaining), self)
                            .expect("serialise");
                    } else {
                        let time = self.start.elapsed();
                        trace!(self.ctx.log(), "Pinger is done! Run took {:?}", time);
                        let promise = self.done.take().expect("No promise to reply to?");
                        promise.fulfill(time).expect("Promise was dropped");
                    }
                }
                Err(e) => {
                    error!(self.ctx.log(), "Error deserialising Pong: {:?}", e);
                    unimplemented!("Pinger received unexpected message!")
                }
            }
        }
    }

    #[derive(ComponentDefinition)]
    pub struct Ponger {
        ctx: ComponentContext<Self>,
    }

    impl Ponger {
        pub fn new() -> Ponger {
            Ponger {
                ctx: ComponentContext::new(),
            }
        }
    }

    impl Provide<ControlPort> for Ponger {
        fn handle(&mut self, event: ControlEvent) -> () {
            match event {
                ControlEvent::Start => {
                    debug!(self.ctx.log(), "Starting Ponger");
                }
                e => {
                    debug!(self.ctx.log(), "Ponger got control event: {:?}", e);
                }
            }
        }
    }

    impl Actor for Ponger {
        type Message = Never;

        fn receive_local(&mut self, _msg: Self::Message) -> () {
            unimplemented!("Never can't be instantiated!");
        }

        fn receive_network(&mut self, msg: NetMessage) -> () {
            trace!(self.ctx.log(), "Ponger received msg {:?}", msg, );
            let sender = msg.sender;
            match msg.data.try_deserialise::<Ping, Ping>() {
                Ok(ping) => {
                    trace!(self.ctx.log(), "Ponger got Ping!");
                    sender.tell_serialised(Pong::new(ping.index), self)
                        .expect("serialise");
                }
                Err(e) => {
                    error!(self.ctx.log(), "Error deserialising Ping: {:?}", e);
                    unimplemented!("Ponger received unexpected message!");
                }
            }
        }
    }

    #[derive(Debug, Clone, Copy)]
    pub struct Ping {
        index: u64,
    }

    impl Ping {
        pub const SERID: SerId = 42;

        pub fn new(index: u64) -> Ping {
            Ping { index }
        }
    }

    impl Serialisable for Ping {
        #[inline(always)]
        fn ser_id(&self) -> SerId {
            Ping::SERID
        }

        fn size_hint(&self) -> Option<usize> {
            Some(8)
        }

        fn serialise(&self, buf: &mut dyn BufMut) -> Result<(), SerError> {
            buf.put_u64(self.index);
            Ok(())
        }

        fn local(self: Box<Self>) -> Result<Box<dyn Any + Send>, Box<dyn Serialisable>> {
            Ok(self)
        }
    }

    impl Deserialiser<Ping> for Ping {
        const SER_ID: SerId = Ping::SERID;

        fn deserialise(buf: &mut dyn Buf) -> Result<Ping, SerError> {
            let index = buf.get_u64();
            Ok(Ping::new(index))
        }
    }

    #[derive(Debug, Clone, Copy)]
    pub struct Pong {
        index: u64,
    }

    impl Pong {
        pub const SERID: SerId = 43;

        pub fn new(index: u64) -> Pong {
            Pong { index }
        }
    }

    impl Serialisable for Pong {
        #[inline(always)]
        fn ser_id(&self) -> SerId {
            Pong::SERID
        }

        fn size_hint(&self) -> Option<usize> {
            Some(8)
        }

        fn serialise(&self, buf: &mut dyn BufMut) -> Result<(), SerError> {
            buf.put_u64(self.index);
            Ok(())
        }

        fn local(self: Box<Self>) -> Result<Box<dyn Any + Send>, Box<dyn Serialisable>> {
            Ok(self)
        }
    }

    impl Deserialiser<Pong> for Pong {
        const SER_ID: SerId = Pong::SERID;

        fn deserialise(buf: &mut dyn Buf) -> Result<Pong, SerError> {
            let index = buf.get_u64();
            Ok(Pong::new(index))
        }
    }
}

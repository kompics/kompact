use criterion::{black_box, criterion_group, criterion_main, Bencher, Criterion, BenchmarkId};
use std::time::{Duration, Instant};
//use kompact::*;
use kompact::prelude::*;
//use kompact::default_components::DeadletterBox;

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
        group.bench_with_input(BenchmarkId::from_parameter(threads), &threads, ping_pong_latency_static_threads);
    }
    group.finish();
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
    let ponger_path = ponger_f
        .wait_timeout(timeout)
        .expect("Ponger never registered")
        .expect("Ponger failed to register!");

    let (pinger, pinger_f) = sys1.create_and_register(move || pinger_func(ponger_path));
    let _pinger_path = pinger_f
        .wait_timeout(timeout)
        .expect("Pinger never registered")
        .expect("Pinger failed to register!");

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
    KompactSystem::new(cfg).expect("KompactSystem")
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
    let ponger_path = ponger_f
        .wait_timeout(timeout)
        .expect("Ponger never registered")
        .expect("Ponger failed to register!");

    let (pinger, pinger_f) = sys1.create_and_register(move || pinger_func(ponger_path));
    let _pinger_path = pinger_f
        .wait_timeout(timeout)
        .expect("Pinger never registered")
        .expect("Pinger failed to register!");

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

criterion_group!(latency_benches, kompact_network_latency, latch_overhead);
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
                self.ponger.tell(Ping::EVENT, self);
            }
        }
    }
    impl Actor for Pinger {
        fn receive_local(&mut self, sender: ActorRef, msg: &dyn Any) -> () {
            warn!(
                self.ctx.log(),
                "Ponger received {:?} (type_id={:?}) from {}, but doesn't handle such things!",
                msg,
                msg.type_id(),
                sender
            );
        }

        fn receive_message(&mut self, sender: ActorPath, ser_id: u64, _buf: &mut dyn Buf) -> () {
            trace!(
                self.ctx.log(),
                "Pinger received buffer with id {:?} from {}",
                ser_id,
                sender
            );
            match ser_id {
                Pong::SER_ID => {
                    self.remaining_recv -= 1u64;
                    trace!(self.ctx.log(), "Pinger got Pong #{}!", self.remaining_recv);
                    if self.remaining_recv <= 0u64 {
                        let time = self.start.elapsed();
                        trace!(self.ctx.log(), "Pinger is done! Run took {:?}", time);
                        let promise = self.done.take().expect("No promise to reply to?");
                        promise.fulfill(time).expect("Promise was dropped");;
                    }
                }
                _ => unimplemented!("Ponger received unexpected message!"),
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
        fn receive_local(&mut self, sender: ActorRef, msg: &dyn Any) -> () {
            warn!(
                self.ctx.log(),
                "Ponger received {:?} (type_id={:?}) from {}, but doesn't handle such things!",
                msg,
                msg.type_id(),
                sender
            );
        }

        fn receive_message(&mut self, sender: ActorPath, ser_id: u64, _buf: &mut dyn Buf) -> () {
            trace!(
                self.ctx.log(),
                "Ponger received buffer with id {:?} from {}",
                ser_id,
                sender
            );
            match ser_id {
                Ping::SER_ID => {
                    trace!(self.ctx.log(), "Ponger got Ping!");
                    sender.tell(Pong::EVENT, self);
                }
                _ => unimplemented!("Ponger received unexpectd message!"),
            }
        }
    }

    #[derive(Debug, Clone, Copy)]
    pub struct Ping;
    impl Ping {
        pub const EVENT: Ping = Ping {};
        pub const SER_ID: u64 = 42;
    }
    impl Serialisable for Ping {
        #[inline(always)]
        fn serid(&self) -> u64 {
            Ping::SER_ID
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
        fn deserialise(_buf: &mut dyn Buf) -> Result<Ping, SerError> {
            Ok(Ping)
        }
    }

    #[derive(Debug, Clone, Copy)]
    pub struct Pong;
    impl Pong {
        pub const EVENT: Pong = Pong {};
        pub const SER_ID: u64 = 43;
    }
    impl Serialisable for Pong {
        #[inline(always)]
        fn serid(&self) -> u64 {
            Pong::SER_ID
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
                self.ponger.tell(Ping::new(self.remaining_send), self);
            }
        }
    }
    impl Actor for Pinger {
        fn receive_local(&mut self, sender: ActorRef, msg: &dyn Any) -> () {
            warn!(
                self.ctx.log(),
                "Ponger received {:?} (type_id={:?}) from {}, but doesn't handle such things!",
                msg,
                msg.type_id(),
                sender
            );
        }

        fn receive_message(&mut self, sender: ActorPath, ser_id: u64, buf: &mut dyn Buf) -> () {
            trace!(
                self.ctx.log(),
                "Pinger received buffer with id {:?} from {}",
                ser_id,
                sender
            );
            match ser_id {
                Pong::SER_ID => {
                    if let Ok(_pong) = Pong::deserialise(buf) {
                        self.remaining_recv -= 1u64;
                        trace!(self.ctx.log(), "Pinger got Pong #{}!", self.remaining_recv);
                        if self.remaining_recv <= 0u64 {
                            let time = self.start.elapsed();
                            trace!(self.ctx.log(), "Pinger is done! Run took {:?}", time);
                            let promise = self.done.take().expect("No promise to reply to?");
                            promise.fulfill(time).expect("Promise was dropped");;
                        }
                    } else {
                        error!(self.ctx.log(), "Could not deserialise Pong message!");
                    }
                }
                _ => unimplemented!("Ponger received unexpected message!"),
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
        fn receive_local(&mut self, sender: ActorRef, msg: &dyn Any) -> () {
            warn!(
                self.ctx.log(),
                "Ponger received {:?} (type_id={:?}) from {}, but doesn't handle such things!",
                msg,
                msg.type_id(),
                sender
            );
        }

        fn receive_message(&mut self, sender: ActorPath, ser_id: u64, buf: &mut dyn Buf) -> () {
            trace!(
                self.ctx.log(),
                "Ponger received buffer with id {:?} from {}",
                ser_id,
                sender
            );
            match ser_id {
                Ping::SER_ID => {
                    if let Ok(ping) = Ping::deserialise(buf) {
                        trace!(self.ctx.log(), "Ponger got Ping!");
                        sender.tell(Pong::new(ping.index), self);
                    } else {
                        error!(self.ctx.log(), "Could not deserialise Ping message!");
                    }
                }
                _ => unimplemented!("Ponger received unexpectd message!"),
            }
        }
    }

    #[derive(Debug, Clone, Copy)]
    pub struct Ping {
        index: u64,
    }
    impl Ping {
        pub const SER_ID: u64 = 42;

        pub fn new(index: u64) -> Ping {
            Ping { index }
        }
    }
    impl Serialisable for Ping {
        #[inline(always)]
        fn serid(&self) -> u64 {
            Ping::SER_ID
        }

        fn size_hint(&self) -> Option<usize> {
            Some(8)
        }

        fn serialise(&self, buf: &mut dyn BufMut) -> Result<(), SerError> {
            buf.put_u64_be(self.index);
            Ok(())
        }

        fn local(self: Box<Self>) -> Result<Box<dyn Any + Send>, Box<dyn Serialisable>> {
            Ok(self)
        }
    }
    impl Deserialiser<Ping> for Ping {
        fn deserialise(buf: &mut dyn Buf) -> Result<Ping, SerError> {
            let index = buf.get_u64_be();
            Ok(Ping::new(index))
        }
    }

    #[derive(Debug, Clone, Copy)]
    pub struct Pong {
        index: u64,
    }
    impl Pong {
        pub const SER_ID: u64 = 43;

        pub fn new(index: u64) -> Pong {
            Pong { index }
        }
    }
    impl Serialisable for Pong {
        #[inline(always)]
        fn serid(&self) -> u64 {
            Pong::SER_ID
        }

        fn size_hint(&self) -> Option<usize> {
            Some(8)
        }

        fn serialise(&self, buf: &mut dyn BufMut) -> Result<(), SerError> {
            buf.put_u64_be(self.index);
            Ok(())
        }

        fn local(self: Box<Self>) -> Result<Box<dyn Any + Send>, Box<dyn Serialisable>> {
            Ok(self)
        }
    }
    impl Deserialiser<Pong> for Pong {
        fn deserialise(buf: &mut dyn Buf) -> Result<Pong, SerError> {
            let index = buf.get_u64_be();
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
            self.ponger.tell(Ping::EVENT, self);
        }
    }
    impl Actor for Pinger {
        fn receive_local(&mut self, sender: ActorRef, msg: &dyn Any) -> () {
            warn!(
                self.ctx.log(),
                "Ponger received {:?} (type_id={:?}) from {}, but doesn't handle such things!",
                msg,
                msg.type_id(),
                sender
            );
        }

        fn receive_message(&mut self, sender: ActorPath, ser_id: u64, _buf: &mut dyn Buf) -> () {
            trace!(
                self.ctx.log(),
                "Pinger received buffer with id {:?} from {}",
                ser_id,
                sender
            );
            match ser_id {
                Pong::SER_ID => {
                    self.remaining -= 1u64;
                    trace!(self.ctx.log(), "Pinger got Pong #{}!", self.remaining);
                    if self.remaining > 0u64 {
                        sender.tell(Ping::EVENT, self);
                    } else {
                        let time = self.start.elapsed();
                        trace!(self.ctx.log(), "Pinger is done! Run took {:?}", time);
                        let promise = self.done.take().expect("No promise to reply to?");
                        promise.fulfill(time).expect("Promise was dropped");;
                    }
                }
                _ => unimplemented!("Ponger received unexpected message!"),
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
        fn receive_local(&mut self, sender: ActorRef, msg: &dyn Any) -> () {
            warn!(
                self.ctx.log(),
                "Ponger received {:?} (type_id={:?}) from {}, but doesn't handle such things!",
                msg,
                msg.type_id(),
                sender
            );
        }

        fn receive_message(&mut self, sender: ActorPath, ser_id: u64, _buf: &mut dyn Buf) -> () {
            trace!(
                self.ctx.log(),
                "Ponger received buffer with id {:?} from {}",
                ser_id,
                sender
            );
            match ser_id {
                Ping::SER_ID => {
                    trace!(self.ctx.log(), "Ponger got Ping!");
                    sender.tell(Pong::EVENT, self);
                }
                _ => unimplemented!("Ponger received unexpectd message!"),
            }
        }
    }

    #[derive(Debug, Clone, Copy)]
    pub struct Ping;
    impl Ping {
        pub const EVENT: Ping = Ping {};
        pub const SER_ID: u64 = 42;
    }
    impl Serialisable for Ping {
        #[inline(always)]
        fn serid(&self) -> u64 {
            Ping::SER_ID
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
        fn deserialise(_buf: &mut dyn Buf) -> Result<Ping, SerError> {
            Ok(Ping)
        }
    }

    #[derive(Debug, Clone, Copy)]
    pub struct Pong;
    impl Pong {
        pub const EVENT: Pong = Pong {};
        pub const SER_ID: u64 = 43;
    }
    impl Serialisable for Pong {
        #[inline(always)]
        fn serid(&self) -> u64 {
            Pong::SER_ID
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
        fn deserialise(_buf: &mut dyn Buf) -> Result<Pong, SerError> {
            Ok(Pong)
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
            self.ponger.tell(Ping::new(self.remaining), self);
        }
    }
    impl Actor for Pinger {
        fn receive_local(&mut self, sender: ActorRef, msg: &dyn Any) -> () {
            warn!(
                self.ctx.log(),
                "Ponger received {:?} (type_id={:?}) from {}, but doesn't handle such things!",
                msg,
                msg.type_id(),
                sender
            );
        }

        fn receive_message(&mut self, sender: ActorPath, ser_id: u64, buf: &mut dyn Buf) -> () {
            trace!(
                self.ctx.log(),
                "Pinger received buffer with id {:?} from {}",
                ser_id,
                sender
            );
            match ser_id {
                Pong::SER_ID => {
                    if let Ok(_pong) = Pong::deserialise(buf) {
                        self.remaining -= 1u64;
                        trace!(self.ctx.log(), "Pinger got Pong #{}!", self.remaining);
                        if self.remaining > 0u64 {
                            sender.tell(Ping::new(self.remaining), self);
                        } else {
                            let time = self.start.elapsed();
                            trace!(self.ctx.log(), "Pinger is done! Run took {:?}", time);
                            let promise = self.done.take().expect("No promise to reply to?");
                            promise.fulfill(time).expect("Promise was dropped");
                        }
                    } else {
                        error!(self.ctx.log(), "Could not deserialise Pong message!");
                    }
                }
                _ => unimplemented!("Ponger received unexpected message!"),
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
        fn receive_local(&mut self, sender: ActorRef, msg: &dyn Any) -> () {
            warn!(
                self.ctx.log(),
                "Ponger received {:?} (type_id={:?}) from {}, but doesn't handle such things!",
                msg,
                msg.type_id(),
                sender
            );
        }

        fn receive_message(&mut self, sender: ActorPath, ser_id: u64, buf: &mut dyn Buf) -> () {
            trace!(
                self.ctx.log(),
                "Ponger received buffer with id {:?} from {}",
                ser_id,
                sender
            );
            match ser_id {
                Ping::SER_ID => {
                    if let Ok(ping) = Ping::deserialise(buf) {
                        trace!(self.ctx.log(), "Ponger got Ping!");
                        sender.tell(Pong::new(ping.index), self);
                    } else {
                        error!(self.ctx.log(), "Could not deserialise Ping message!");
                    }
                }
                _ => unimplemented!("Ponger received unexpectd message!"),
            }
        }
    }

    #[derive(Debug, Clone, Copy)]
    pub struct Ping {
        index: u64,
    }
    impl Ping {
        pub const SER_ID: u64 = 42;

        pub fn new(index: u64) -> Ping {
            Ping { index }
        }
    }
    impl Serialisable for Ping {
        #[inline(always)]
        fn serid(&self) -> u64 {
            Ping::SER_ID
        }

        fn size_hint(&self) -> Option<usize> {
            Some(8)
        }

        fn serialise(&self, buf: &mut dyn BufMut) -> Result<(), SerError> {
            buf.put_u64_be(self.index);
            Ok(())
        }

        fn local(self: Box<Self>) -> Result<Box<dyn Any + Send>, Box<dyn Serialisable>> {
            Ok(self)
        }
    }
    impl Deserialiser<Ping> for Ping {
        fn deserialise(buf: &mut dyn Buf) -> Result<Ping, SerError> {
            let index = buf.get_u64_be();
            Ok(Ping::new(index))
        }
    }

    #[derive(Debug, Clone, Copy)]
    pub struct Pong {
        index: u64,
    }
    impl Pong {
        pub const SER_ID: u64 = 43;

        pub fn new(index: u64) -> Pong {
            Pong { index }
        }
    }
    impl Serialisable for Pong {
        #[inline(always)]
        fn serid(&self) -> u64 {
            Pong::SER_ID
        }

        fn size_hint(&self) -> Option<usize> {
            Some(8)
        }

        fn serialise(&self, buf: &mut dyn BufMut) -> Result<(), SerError> {
            buf.put_u64_be(self.index);
            Ok(())
        }

        fn local(self: Box<Self>) -> Result<Box<dyn Any + Send>, Box<dyn Serialisable>> {
            Ok(self)
        }
    }
    impl Deserialiser<Pong> for Pong {
        fn deserialise(buf: &mut dyn Buf) -> Result<Pong, SerError> {
            let index = buf.get_u64_be();
            Ok(Pong::new(index))
        }
    }
}

use criterion::{criterion_group, criterion_main, Criterion};
use kompact::prelude::*;

#[derive(Debug, Clone)]
pub struct Ping;

pub const PING: Ping = Ping;

impl From<Ping> for &'static Ping {
    fn from(_p: Ping) -> Self {
        &PING
    }
}

struct TestPort;

impl Port for TestPort {
    type Indication = ();
    type Request = &'static Ping;
}

#[derive(ComponentDefinition)]
pub struct TestActor {
    ctx: ComponentContext<Self>,
    testp: ProvidedPort<TestPort>,
}

impl TestActor {
    pub fn new() -> TestActor {
        TestActor {
            ctx: ComponentContext::uninitialised(),
            testp: ProvidedPort::uninitialised(),
        }
    }
}

ignore_lifecycle!(TestActor);

impl Provide<TestPort> for TestActor {
    fn handle(&mut self, _event: &'static Ping) -> Handled {
        Handled::Ok // discard
    }
}

impl Actor for TestActor {
    type Message = &'static Ping;

    fn receive_local(&mut self, _msg: Self::Message) -> Handled {
        Handled::Ok // discard
    }

    fn receive_network(&mut self, _msg: NetMessage) -> Handled {
        Handled::Ok // discard
    }
}

pub struct PingSer;

pub const PING_SER: PingSer = PingSer {};

impl Serialiser<Ping> for PingSer {
    fn ser_id(&self) -> SerId {
        42 // because why not^^
    }

    fn size_hint(&self) -> Option<usize> {
        Some(0)
    }

    fn serialise(&self, _v: &Ping, _buf: &mut dyn BufMut) -> Result<(), SerError> {
        Result::Ok(())
    }
}

pub fn clone_benches(c: &mut Criterion) {
    let mut g = c.benchmark_group("Clone Benches");
    g.bench_function("bench clone ActorRef", |b| tests::bench_clone_actor_ref(b));
    g.bench_function("bench clone Recipient", |b| tests::bench_clone_recipient(b));
    g.bench_function("bench clone ActorPath", |b| {
        tests::bench_clone_actor_path(b)
    });
    g.finish();
}

pub fn tell_benches(c: &mut Criterion) {
    let mut g = c.benchmark_group("Tell/Trigger Benches");
    g.bench_function("bench tell ActorRef", |b| tests::bench_tell_actor_ref(b));
    g.bench_function("bench tell Recipient", |b| tests::bench_tell_recipient(b));
    g.bench_function("bench tell ActorRef (Strong)", |b| {
        tests::bench_tell_actor_ref_strong(b)
    });
    g.bench_function("bench trigger Port", |b| tests::bench_trigger_port(b));
    //g.bench_function("bench tell ActorPath", |b| tests::bench_tell_actor_path(b));
    g.finish();
}

mod tests {
    use super::*;
    use criterion::Bencher;
    use std::time::Duration;

    pub fn bench_clone_recipient(b: &mut Bencher) {
        let sys = KompactConfig::default().build().expect("System");
        let tester = sys.create(TestActor::new);
        let tester_ref = tester.actor_ref();
        let tester_recipient: Recipient<Ping> = tester_ref.recipient();
        let mut cloned_recipient = tester_recipient.clone();
        b.iter(|| {
            cloned_recipient = tester_recipient.clone();
        });
        drop(cloned_recipient);
        drop(tester_recipient);
        drop(tester_ref);
        drop(tester);
        sys.shutdown().expect("System didn't shut down :(");
    }

    pub fn bench_tell_recipient(b: &mut Bencher) {
        let sys = KompactConfig::default().build().expect("System");
        let tester = sys.create(TestActor::new);
        let tester_ref = tester.actor_ref();
        let tester_recipient: Recipient<Ping> = tester_ref.recipient();
        b.iter(|| {
            tester_recipient.tell(PING);
        });
        drop(tester_ref);
        drop(tester);
        sys.shutdown().expect("System didn't shut down :(");
    }

    pub fn bench_clone_actor_ref(b: &mut Bencher) {
        let sys = KompactConfig::default().build().expect("System");
        let tester = sys.create(TestActor::new);
        let tester_ref = tester.actor_ref();
        let mut cloned_ref = tester_ref.clone();
        b.iter(|| {
            cloned_ref = tester_ref.clone();
        });
        drop(tester_ref);
        drop(cloned_ref);
        drop(tester);
        sys.shutdown().expect("System didn't shut down :(");
    }

    pub fn bench_tell_actor_ref(b: &mut Bencher) {
        let sys = KompactConfig::default().build().expect("System");
        let tester = sys.create(TestActor::new);
        let tester_ref = tester.actor_ref();
        b.iter(|| {
            tester_ref.tell(&PING);
        });
        drop(tester_ref);
        drop(tester);
        sys.shutdown().expect("System didn't shut down :(");
    }

    pub fn bench_tell_actor_ref_strong(b: &mut Bencher) {
        let sys = KompactConfig::default().build().expect("System");
        let tester = sys.create(TestActor::new);
        let tester_ref = tester.actor_ref().hold().expect("Live Ref");
        b.iter(|| {
            tester_ref.tell(&PING);
        });
        drop(tester_ref);
        drop(tester);
        sys.shutdown().expect("System didn't shut down :(");
    }

    pub fn bench_trigger_port(b: &mut Bencher) {
        let sys = KompactConfig::default().build().expect("System");
        let tester = sys.create(TestActor::new);
        let test_port = tester.on_definition(|c| c.testp.share());
        b.iter(|| {
            sys.trigger_r(&PING, &test_port);
        });
        drop(test_port);
        drop(tester);
        sys.shutdown().expect("System didn't shut down :(");
    }

    pub fn bench_clone_actor_path(b: &mut Bencher) {
        let sys = {
            let mut cfg = KompactConfig::new();
            cfg.system_components(DeadletterBox::new, NetworkConfig::default().build());
            cfg.build().expect("KompactSystem")
        };
        let (tester, testerf) = sys.create_and_register(TestActor::new);
        testerf.wait_expect(Duration::from_millis(1000), "Tester failed to register!");
        let unique_path = ActorPath::Unique(UniquePath::with_system(
            sys.system_path(),
            tester.id().clone(),
        ));
        let mut cloned_path = unique_path.clone();
        b.iter(|| {
            cloned_path = unique_path.clone();
        });
        drop(cloned_path);
        drop(unique_path);
        drop(tester);
        sys.shutdown().expect("System didn't shut down :(");
    }

    #[allow(dead_code)]
    pub fn bench_tell_actor_path(b: &mut Bencher) {
        let sys = {
            let mut cfg = KompactConfig::new();
            cfg.system_components(DeadletterBox::new, NetworkConfig::default().build());
            cfg.build().expect("KompactSystem")
        };
        let (tester, testerf) = sys.create_and_register(TestActor::new);
        testerf.wait_expect(Duration::from_millis(1000), "Tester failed to register!");
        let unique_path = ActorPath::Unique(UniquePath::with_system(
            sys.system_path(),
            tester.id().clone(),
        ));
        b.iter(|| {
            unique_path.tell((PING, PING_SER), &sys);
        });
        drop(unique_path);
        drop(tester);
        sys.shutdown().expect("System didn't shut down :(");
    }
}

criterion_group!(actor_benches, clone_benches, tell_benches);
criterion_main!(actor_benches);

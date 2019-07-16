use kompact::prelude::*;
use kompact::*;

#[derive(Debug, Clone)]
pub struct Ping;
pub const PING: Ping = Ping;

struct TestPort;
impl Port for TestPort {
    type Indication = ();
    type Request = &'static Ping;
}

#[derive(ComponentDefinition)]
pub struct TestActor {
    ctx: ComponentContext<Self>,
    testp: ProvidedPort<TestPort, Self>,
}
impl TestActor {
    pub fn new() -> TestActor {
        TestActor {
            ctx: ComponentContext::new(),
            testp: ProvidedPort::new(),
        }
    }
}

impl Provide<ControlPort> for TestActor {
    fn handle(&mut self, _event: ControlEvent) -> () {
        // ignore
    }
}

impl Provide<TestPort> for TestActor {
    fn handle(&mut self, _event: &'static Ping) -> () {
        // discard
    }
}

impl Actor for TestActor {
    fn receive_local(&mut self, _sender: ActorRef, _msg: &Any) -> () {
        // discard
    }
    fn receive_message(&mut self, _sender: ActorPath, _sid: u64, _buf: &mut Buf) -> () {
        // discard
    }
}

struct PingSer;
const PING_SER: PingSer = PingSer {};
impl Serialiser<Ping> for PingSer {
    fn serid(&self) -> u64 {
        42 // because why not^^
    }
    fn size_hint(&self) -> Option<usize> {
        Some(0)
    }
    fn serialise(&self, v: &Ping, buf: &mut BufMut) -> Result<(), SerError> {
        Result::Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use test::Bencher;

    #[bench]
    fn bench_clone_actor_ref(b: &mut Bencher) {
        let sys = KompactConfig::default().build().expect("System");
        let tester = sys.create_and_start(TestActor::new);
        let tester_ref = tester.actor_ref();
        let mut cloned_ref = tester_ref.clone();
        b.iter(|| {
            cloned_ref = tester_ref.clone();
        });
    }

    #[bench]
    fn bench_tell_actor_ref(b: &mut Bencher) {
        let sys = KompactConfig::default().build().expect("System");
        let tester = sys.create_and_start(TestActor::new);
        let tester_ref = tester.actor_ref();
        b.iter(|| {
            tester_ref.tell(&PING, &sys);
        });
    }

    #[bench]
    fn bench_tell_actor_ref_sys(b: &mut Bencher) {
        let sys = KompactConfig::default().build().expect("System");
        let tester = sys.create_and_start(TestActor::new);
        let tester_ref = tester.actor_ref();
        let sys_ref = sys.actor_ref();
        b.iter(|| {
            tester_ref.tell(&PING, &sys_ref);
        });
    }

    #[bench]
    fn bench_tell_actor_ref_strong_sys(b: &mut Bencher) {
        let sys = KompactConfig::default().build().expect("System");
        let tester = sys.create_and_start(TestActor::new);
        let tester_ref = tester.actor_ref().hold().expect("Live Ref");
        let sys_ref = sys.actor_ref();
        b.iter(|| {
            tester_ref.tell(&PING, &sys_ref);
        });
    }

    #[bench]
    fn bench_triggeer_port(b: &mut Bencher) {
        let sys = KompactConfig::default().build().expect("System");
        let tester = sys.create_and_start(TestActor::new);
        let test_port = tester.on_definition(|c| c.testp.share());
        b.iter(|| {
            sys.trigger_r(&PING, &test_port);
        });
    }

    #[bench]
    fn bench_clone_actor_path(b: &mut Bencher) {
        let sys = {
            let mut cfg = KompactConfig::new();
            cfg.system_components(DeadletterBox::new, NetworkConfig::default().build());
            KompactSystem::new(cfg).expect("KompactSystem")
        };
        let (tester, testerf) = sys.create_and_register(TestActor::new);
        testerf
            .wait_timeout(Duration::from_millis(1000))
            .expect("Tester never registered!")
            .expect("Tester failed to register!");
        let unique_path = ActorPath::Unique(UniquePath::with_system(
            sys.system_path(),
            tester.id().clone(),
        ));
        let mut cloned_path = unique_path.clone();
        b.iter(|| {
            cloned_path = unique_path.clone();
        });
    }

    #[bench]
    fn bench_tell_actor_path(b: &mut Bencher) {
        let sys = {
            let mut cfg = KompactConfig::new();
            cfg.system_components(DeadletterBox::new, NetworkConfig::default().build());
            KompactSystem::new(cfg).expect("KompactSystem")
        };
        let (tester, testerf) = sys.create_and_register(TestActor::new);
        testerf
            .wait_timeout(Duration::from_millis(1000))
            .expect("Tester never registered!")
            .expect("Tester failed to register!");
        let unique_path = ActorPath::Unique(UniquePath::with_system(
            sys.system_path(),
            tester.id().clone(),
        ));
        b.iter(|| {
            unique_path.tell((PING, PING_SER), &sys);
        });
    }
}

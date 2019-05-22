#![allow(unused_parens)]
#![feature(specialization)]
#![feature(unsized_locals)]
#![feature(drain_filter)]

pub use self::actors::*;
pub use self::component::*;
use self::default_components::*;
pub use self::dispatch::*;
pub use self::lifecycle::*;
pub use self::ports::*;
pub use self::runtime::*;
pub use self::serialisation::*;
pub use self::timer_manager::*;
pub use self::utils::*;
pub use bytes::Buf;
pub use crossbeam_queue::SegQueue as ConcurrentQueue;
pub use kompact_actor_derive::*;
pub use kompact_component_derive::*;
pub use slog::{crit, debug, error, info, o, trace, warn, Drain, Fuse, Logger};
pub use slog_async::Async;
pub use std::any::Any;
pub use std::convert::{From, Into};

mod actors;
mod component;
pub mod default_components;
mod dispatch;
mod lifecycle;
pub mod messaging;
pub mod net;
mod ports;
mod runtime;
mod serialisation;
mod supervision;
pub mod timer;
mod timer_manager;
mod utils;

pub mod prelude {
    pub use bytes::{Buf, BufMut, IntoBuf};

    pub use crate::actors::{Actor, ActorPath, ActorRef, NamedPath, UniquePath};
    pub use crate::component::{
        Component, ComponentContext, ComponentDefinition, CoreContainer, ExecuteResult,
    };
    pub use crate::component::{Provide, Require};
    pub use crate::lifecycle::{ControlEvent, ControlPort};
    pub use crate::runtime::{KompactConfig, KompactSystem};
    pub use crate::Any;

    pub use crate::default_components::{CustomComponents, DeadletterBox};
    pub use crate::dispatch::{NetworkConfig, NetworkDispatcher};
    pub use crate::messaging::{
        DispatchEnvelope, MsgEnvelope, PathResolvable, ReceiveEnvelope, RegistrationError,
    };
    pub use crate::serialisation::{
        Deserialisable, Deserialiser, SerError, Serialisable, Serialiser,
    };
}

pub type KompactLogger = Logger<std::sync::Arc<Fuse<Async>>>;

#[cfg(test)]
mod tests {

    use std::{thread, time};
    //use futures::{Future, future};
    //use futures_cpupool::CpuPool;
    use super::*;
    use bytes::Buf;
    use std::any::Any;
    use std::sync::Arc;
    use std::time::Duration;

    struct TestPort;

    impl Port for TestPort {
        type Indication = Arc<String>;
        type Request = Arc<u64>;
    }

    #[derive(ComponentDefinition, Actor)]
    struct TestComponent {
        ctx: ComponentContext<TestComponent>,
        test_port: ProvidedPort<TestPort, TestComponent>,
        counter: u64,
    }

    impl TestComponent {
        fn new() -> TestComponent {
            TestComponent {
                ctx: ComponentContext::new(),
                counter: 0,
                test_port: ProvidedPort::new(),
            }
        }
    }

    impl Provide<ControlPort> for TestComponent {
        fn handle(&mut self, event: ControlEvent) -> () {
            match event {
                ControlEvent::Start => {
                    info!(self.ctx.log(), "Starting TestComponent");
                }
                _ => (), // ignore
            }
        }
    }

    impl Provide<TestPort> for TestComponent {
        fn handle(&mut self, event: Arc<u64>) -> () {
            self.counter += *event;
            self.test_port.trigger(Arc::new(String::from("Test")));
        }
    }

    #[derive(ComponentDefinition)]
    struct RecvComponent {
        ctx: ComponentContext<RecvComponent>,
        test_port: RequiredPort<TestPort, RecvComponent>,
        last_string: String,
    }

    impl RecvComponent {
        fn new() -> RecvComponent {
            RecvComponent {
                ctx: ComponentContext::new(),
                test_port: RequiredPort::new(),
                last_string: String::from("none ;("),
            }
        }
    }

    //const RECV: &str = "Msg Received";

    impl Actor for RecvComponent {
        fn receive_local(&mut self, sender: ActorRef, msg: &Any) -> () {
            info!(self.ctx.log(), "RecvComponent received {:?}", msg);
            if let Some(s) = msg.downcast_ref::<&str>() {
                self.last_string = s.to_string();
            }
            sender.tell(&"Msg Received", self);
            //sender.actor_path().tell("Msg Received", self);
        }
        fn receive_message(&mut self, sender: ActorPath, _ser_id: u64, _buf: &mut Buf) -> () {
            error!(self.ctx.log(), "Got unexpected message from {}", sender);
            unimplemented!(); // shouldn't happen during the test
        }
    }

    impl Provide<ControlPort> for RecvComponent {
        fn handle(&mut self, event: ControlEvent) -> () {
            match event {
                ControlEvent::Start => {
                    info!(self.ctx.log(), "Starting RecvComponent");
                }
                _ => (), // ignore
            }
        }
    }

    impl Require<TestPort> for RecvComponent {
        fn handle(&mut self, event: Arc<String>) -> () {
            info!(self.ctx.log(), "Got event {}", event.as_ref());
            self.last_string = event.as_ref().clone();
        }
    }

    #[test]
    fn default_settings() {
        //let pool = ThreadPool::new(2);
        let system = KompactConfig::default().build().expect("KompactSystem");

        test_with_system(system);
    }

    #[test]
    fn custom_settings() {
        //let pool = ThreadPool::new(2);
        let mut settings = KompactConfig::new();
        settings
            .threads(4)
            .scheduler(|t| executors::crossbeam_channel_pool::ThreadPool::new(t));
        let system = KompactSystem::new(settings).expect("KompactSystem");
        test_with_system(system);
    }

    fn test_with_system(system: KompactSystem) -> () {
        let tc = system.create(TestComponent::new);
        let rc = system.create(RecvComponent::new);
        let rctp = rc.on_definition(|c| c.test_port.share());
        let tctp = tc.on_definition(|c| {
            c.test_port.connect(rctp);
            c.test_port.share()
        });
        system.start(&tc);
        system.start(&rc);
        let msg = Arc::new(1234);
        system.trigger_r(msg, tctp);

        let ten_millis = time::Duration::from_millis(1000);

        thread::sleep(ten_millis);

        tc.on_definition(|c| {
            //println!("Counter is {}", c.counter);
            assert_eq!(c.counter, 1234);
        });

        thread::sleep(ten_millis);

        rc.on_definition(|c| {
            //println!("Last string was {}", c.last_string);
            assert_eq!(c.last_string, String::from("Test"));
        });

        let rcref = rc.actor_ref();
        rcref.tell(&"MsgTest", &system);

        thread::sleep(ten_millis);

        rc.on_definition(|c| {
            //println!("Last string was {}", c.last_string);
            assert_eq!(c.last_string, String::from("MsgTest"));
        });

        system
            .shutdown()
            .expect("Kompact didn't shut down properly");
    }

    #[derive(ComponentDefinition, Actor)]
    struct TimerRecvComponent {
        ctx: ComponentContext<TimerRecvComponent>,
        last_string: String,
    }

    impl TimerRecvComponent {
        fn new() -> TimerRecvComponent {
            TimerRecvComponent {
                ctx: ComponentContext::new(),
                last_string: String::from("none ;("),
            }
        }
    }

    impl Provide<ControlPort> for TimerRecvComponent {
        fn handle(&mut self, event: ControlEvent) -> () {
            match event {
                ControlEvent::Start => {
                    info!(self.ctx.log(), "Starting TimerRecvComponent");
                    self.schedule_once(Duration::from_millis(100), |self_c, _| {
                        self_c.last_string = String::from("TimerTest");
                    });
                }
                _ => (), // ignore
            }
        }
    }

    #[test]
    fn test_timer() -> () {
        let system = KompactConfig::default().build().expect("KompactSystem");
        let trc = system.create_and_start(TimerRecvComponent::new);

        thread::sleep(Duration::from_millis(1000));

        trc.on_definition(|c| {
            //println!("Counter is {}", c.counter);
            assert_eq!(c.last_string, "TimerTest");
        });

        system
            .shutdown()
            .expect("Kompact didn't shut down properly");
    }

    #[derive(ComponentDefinition)]
    struct CounterComponent {
        ctx: ComponentContext<CounterComponent>,
        msg_count: usize,
    }

    impl CounterComponent {
        fn new() -> CounterComponent {
            CounterComponent {
                ctx: ComponentContext::new(),
                msg_count: 0,
            }
        }
    }

    impl Provide<ControlPort> for CounterComponent {
        fn handle(&mut self, event: ControlEvent) -> () {
            match event {
                ControlEvent::Start => {
                    info!(self.ctx.log(), "Starting CounterComponent");
                }
                ControlEvent::Stop => {
                    info!(self.ctx.log(), "Stopping CounterComponent");
                }
                _ => (), // ignore
            }
        }
    }

    impl Actor for CounterComponent {
        fn receive_local(&mut self, _sender: ActorRef, _msg: &Any) -> () {
            info!(self.ctx.log(), "CounterComponent got a message!");
            self.msg_count += 1;
        }
        fn receive_message(&mut self, sender: ActorPath, _ser_id: u64, _buf: &mut Buf) -> () {
            crit!(self.ctx.log(), "Got unexpected message from {}", sender);
            unimplemented!(); // shouldn't happen during the test
        }
    }

    #[test]
    fn test_start_stop() -> () {
        let system = KompactConfig::default().build().expect("KompactSystem");
        let (cc, _) = system.create_and_register(CounterComponent::new);
        let ccref = cc.actor_ref();

        ccref.tell(Box::new(String::from("MsgTest")), &system);

        thread::sleep(Duration::from_millis(1000));

        cc.on_definition(|c| {
            println!("Counter is {}", c.msg_count);
            assert_eq!(c.msg_count, 0); // not yet started
        });

        let f = system.start_notify(&cc);

        f.wait_timeout(Duration::from_millis(1000))
            .expect("Component never started!");

        cc.on_definition(|c| {
            println!("Counter is {}", c.msg_count);
            assert_eq!(c.msg_count, 1);
        });

        let f = system.stop_notify(&cc);
        ccref.tell(Box::new(String::from("MsgTest")), &system);

        f.wait_timeout(Duration::from_millis(1000))
            .expect("Component never stopped!");

        cc.on_definition(|c| {
            println!("Counter is {}", c.msg_count);
            assert_eq!(c.msg_count, 1);
        });

        let f = system.start_notify(&cc);

        f.wait_timeout(Duration::from_millis(1000))
            .expect("Component never started again!");

        cc.on_definition(|c| {
            println!("Counter is {}", c.msg_count);
            assert_eq!(c.msg_count, 2);
        });

        let f = system.kill_notify(cc);

        f.wait_timeout(Duration::from_millis(1000))
            .expect("Component never died!");

        system
            .shutdown()
            .expect("Kompact didn't shut down properly");
    }

    struct CrashPort;

    impl Port for CrashPort {
        type Indication = ();
        type Request = ();
    }

    #[derive(ComponentDefinition)]
    struct CrasherComponent {
        ctx: ComponentContext<CrasherComponent>,
        crash_port: ProvidedPort<CrashPort, CrasherComponent>,
        crash_on_start: bool,
    }

    impl CrasherComponent {
        fn new(crash_on_start: bool) -> CrasherComponent {
            CrasherComponent {
                ctx: ComponentContext::new(),
                crash_port: ProvidedPort::new(),
                crash_on_start,
            }
        }
    }

    impl Provide<ControlPort> for CrasherComponent {
        fn handle(&mut self, event: ControlEvent) -> () {
            match event {
                ControlEvent::Start => {
                    if self.crash_on_start {
                        info!(self.ctx.log(), "Crashing CounterComponent");
                        panic!("Test panic please ignore");
                    } else {
                        info!(self.ctx.log(), "Starting CounterComponent");
                    }
                }
                ControlEvent::Stop => {
                    info!(self.ctx.log(), "Stopping CounterComponent");
                }
                _ => (), // ignore
            }
        }
    }

    impl Provide<CrashPort> for CrasherComponent {
        fn handle(&mut self, _event: ()) -> () {
            info!(self.ctx.log(), "Crashing CounterComponent");
            panic!("Test panic please ignore");
        }
    }

    impl Actor for CrasherComponent {
        fn receive_local(&mut self, _sender: ActorRef, _msg: &Any) -> () {
            info!(self.ctx.log(), "Crashing CounterComponent");
            panic!("Test panic please ignore");
        }
        fn receive_message(&mut self, _sender: ActorPath, _ser_id: u64, _buf: &mut Buf) -> () {
            info!(self.ctx.log(), "Crashing CounterComponent");
            panic!("Test panic please ignore");
        }
    }

    #[test]
    fn test_component_failure() -> () {
        let system = KompactConfig::default().build().expect("KompactSystem");

        {
            // limit scope
            let cc = system.create(|| CrasherComponent::new(true));

            let f = system.start_notify(&cc);

            let res = f.wait_timeout(Duration::from_millis(1000));
            assert!(res.is_err(), "Component should crash on start");
        }

        {
            // limit scope
            let cc = system.create(|| CrasherComponent::new(false));

            let crash_port = cc.on_definition(|def| def.crash_port.share());

            let f = system.start_notify(&cc);

            let res = f.wait_timeout(Duration::from_millis(1000));
            assert!(res.is_ok(), "Component should not crash on start");

            system.trigger_r((), crash_port);

            thread::sleep(Duration::from_millis(1000));

            assert!(cc.is_faulty(), "Component should have crashed.");
        }

        {
            // limit scope
            let cc = system.create(|| CrasherComponent::new(false));

            let ccref = cc.actor_ref();

            let f = system.start_notify(&cc);

            let res = f.wait_timeout(Duration::from_millis(1000));
            assert!(res.is_ok(), "Component should not crash on start");

            ccref.tell(Box::new(()), &system);

            thread::sleep(Duration::from_millis(1000));

            assert!(cc.is_faulty(), "Component should have crashed.");
        }

        system
            .shutdown()
            .expect("Kompact didn't shut down properly");
    }
}

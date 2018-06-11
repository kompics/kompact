#![allow(unused_parens)]
#![feature(try_from)]
#![feature(specialization)]
#![feature(fnbox)]
#![feature(duration_as_u128)]
//extern crate futures;
//extern crate futures_cpupool;
extern crate as_num;
extern crate bytes;
extern crate crossbeam;
extern crate executors;
extern crate num_cpus;
extern crate oncemutex;
extern crate serde;
extern crate uuid;
#[macro_use]
extern crate component_definition_derive;
#[macro_use]
extern crate actor_derive;

pub use self::actors::*;
pub use self::component::*;
use self::default_components::*;
pub use self::lifecycle::*;
pub use self::ports::*;
pub use self::runtime::*;
pub use self::serialisation::*;
pub use self::utils::*;
pub use self::timer_manager::*;
pub use actor_derive::*;
pub use component_definition_derive::*;
pub use std::convert::{From, Into};

mod actors;
mod component;
mod default_components;
mod default_serialisers;
mod lifecycle;
mod ports;
mod runtime;
mod serialisation;
mod utils;
mod timer_manager;
pub mod timer;

#[cfg(test)]
mod tests {

    use std::{thread, time};
    //use futures::{Future, future};
    //use futures_cpupool::CpuPool;
    use super::*;
    use bytes::Buf;
    use default_serialisers::*;
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
                    println!("Starting TestComponent");
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

    impl Actor for RecvComponent {
        fn receive_local(&mut self, sender: ActorRef, msg: Box<Any>) -> () {
            println!("RecvComponent received {:?}", msg);
            if let Ok(s) = msg.downcast::<String>() {
                self.last_string = *s;
            }
            sender.tell(Box::new("Msg Received".to_string()), self);
            sender.actor_path().tell("Msg Received", self);
        }
        fn receive_message(&mut self, sender: ActorPath, ser_id: u64, buf: &mut Buf) -> () {
            // ignore
        }
    }

    impl Provide<ControlPort> for RecvComponent {
        fn handle(&mut self, event: ControlEvent) -> () {
            match event {
                ControlEvent::Start => {
                    println!("Starting RecvComponent");
                }
                _ => (), // ignore
            }
        }
    }

    impl Require<TestPort> for RecvComponent {
        fn handle(&mut self, event: Arc<String>) -> () {
            println!("Got event {}", event.as_ref());
            self.last_string = event.as_ref().clone();
        }
    }

    #[test]
    fn default_settings() {
        //let pool = ThreadPool::new(2);
        let system = KompicsSystem::default();

        test_with_system(system);
    }

    #[test]
    fn custom_settings() {
        //let pool = ThreadPool::new(2);
        let mut settings = KompicsConfig::new();
        settings
            .threads(4)
            .scheduler(|t| executors::threadpool_executor::ThreadPoolExecutor::new(t));
        let system = KompicsSystem::new(settings);

        test_with_system(system);
    }

    fn test_with_system(system: KompicsSystem) -> () {
        let tc = system.create(TestComponent::new);
        let rc = system.create(RecvComponent::new);
        let rctp = rc.on_definition(|c| c.test_port.share());
        let tctp = tc.on_definition(|c| {
            c.test_port.connect(rctp);
            c.test_port.share()
        });
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
        rcref.tell(Box::new(String::from("MsgTest")), &system);

        thread::sleep(ten_millis);

        rc.on_definition(|c| {
            //println!("Last string was {}", c.last_string);
            assert_eq!(c.last_string, String::from("MsgTest"));
        });

        system
            .shutdown()
            .expect("Kompics didn't shut down properly");
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
                    println!("Starting TimerRecvComponent");
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
        let system = KompicsSystem::default();
        let trc = system.create(TimerRecvComponent::new);
        system.start(&trc);

        thread::sleep(Duration::from_millis(1000));

        trc.on_definition(|c| {
            //println!("Counter is {}", c.counter);
            assert_eq!(c.last_string, "TimerTest");
        });

        system
            .shutdown()
            .expect("Kompics didn't shut down properly");
    }
}

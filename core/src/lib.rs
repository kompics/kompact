//extern crate futures;
//extern crate futures_cpupool;
extern crate crossbeam;
extern crate uuid;
extern crate as_num;
extern crate executors;
extern crate num_cpus;
#[macro_use]
extern crate component_definition_derive;

pub use self::ports::*;
pub use self::component::*;
pub use self::utils::*;
pub use self::runtime::*;
pub use self::lifecycle::*;
pub use component_definition_derive::*;

mod ports;
mod component;
mod utils;
mod runtime;
mod lifecycle;

#[cfg(test)]
mod tests {

    use std::{thread, time};
    //use futures::{Future, future};
    //use futures_cpupool::CpuPool;
    use std::sync::Arc;
    use super::*;

    struct TestPort;

    impl Port for TestPort {
        type Indication = Arc<String>;
        type Request = Arc<u64>;
    }

    #[derive(ComponentDefinition)]
    struct TestComponent {
        test_port: ProvidedPort<TestPort, TestComponent>,
        counter: u64,
    }

    impl TestComponent {
        fn new() -> TestComponent {
            TestComponent {
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
                _ => (),// ignore
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
        test_port: RequiredPort<TestPort, RecvComponent>,
        last_string: String,
    }

    impl RecvComponent {
        fn new() -> RecvComponent {
            RecvComponent {
                test_port: RequiredPort::new(),
                last_string: String::from("none ;("),
            }
        }
    }

    impl Provide<ControlPort> for RecvComponent {
        fn handle(&mut self, event: ControlEvent) -> () {
            match event {
                ControlEvent::Start => {
                    println!("Starting RecvComponent");
                }
                _ => (),// ignore
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
    fn it_works() {
        //let pool = ThreadPool::new(2);
        let system = KompicsSystem::new();

        let tc = system.create(TestComponent::new);
        let rc = system.create(RecvComponent::new);
        let rctp = rc.on_definition(|c| c.test_port.share());
        let tctp = tc.on_definition(|c| {
            c.test_port.connect(rctp);
            c.test_port.share()
        });
        let msg = Arc::new(1000);
        system.trigger_r(msg, tctp);

        let ten_millis = time::Duration::from_millis(1000);

        thread::sleep(ten_millis);

        tc.on_definition(|c| {
            println!("Counter is {}", c.counter);
        });


        thread::sleep(ten_millis);

        rc.on_definition(|c| {
            println!("Last string was {}", c.last_string);
        });
        system.shutdown().expect("Kompics didn't shut down properly");
    }
}

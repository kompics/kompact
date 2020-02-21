//! The Kompact message-passing framework provides a hybrid approach
//! between the Kompics component model and the Actor model for writing distributed systems.
//!
//! To get all Kompact related things into scope import `use kompact::prelude::*;` instead of `use kompact::*;`.
//!
//! # Hello World Example
//! ```
//! use kompact::prelude::*;
//!
//! #[derive(ComponentDefinition, Actor)]
//! struct HelloWorldComponent {
//!     ctx: ComponentContext<Self>
//! }
//! impl HelloWorldComponent {
//!     pub fn new() -> HelloWorldComponent {
//!         HelloWorldComponent {
//!             ctx: ComponentContext::new()
//!         }
//!     }
//! }
//! impl Provide<ControlPort> for HelloWorldComponent {
//!     fn handle(&mut self, event: ControlEvent) -> () {
//!            match event {
//!                ControlEvent::Start => {
//!                    info!(self.ctx.log(), "Hello World!");
//!                }
//!                _ => (), // ignore other control events
//!            }
//!       }
//! }
//!
//! let system = KompactConfig::default().build().expect("system");
//! let component = system.create(HelloWorldComponent::new);
//! let start_future = system.start_notify(&component);
//! start_future.wait();
//! system.shutdown().expect("shutdown");
//! ```

#![deny(missing_docs)]
#![allow(unused_parens)]
#![cfg_attr(nightly, feature(never_type))]

#[cfg(feature = "thread_pinning")]
pub use core_affinity::{get_core_ids, CoreId};

// Protocol buffers serialisation support
#[cfg(feature = "protobuf")]
pub use self::serialisation::protobuf_serialisers;
use self::{
    actors::*,
    component::*,
    default_components::*,
    lifecycle::*,
    ports::*,
    runtime::*,
    serialisation::*,
    timer_manager::*,
    utils::*,
};
use crossbeam_queue::SegQueue as ConcurrentQueue;
// The default crate for scheduler implementations
pub use executors;
#[allow(unused_imports)]
use kompact_actor_derive::*;
use kompact_component_derive::*;
#[allow(unused_imports)]
use slog::{crit, debug, error, info, o, trace, warn, Drain, Fuse, Logger};
use slog_async::Async;
use std::convert::{From, Into};

mod actors;
/// Traits and structs for component API and internals
pub mod component;
mod dedicated_scheduler;
/// Default implementations for system components
pub mod default_components;
mod dispatch;
mod lifecycle;
/// Facilities and utilities for dealing with network messages
pub mod messaging;
/// Default networking implementation
pub mod net;
mod ports;
/// Kompact system runtime facilities, such as configuration and schedulers
pub mod runtime;
mod serialisation;
mod supervision;
/// Reusable timer facility internals
pub mod timer;
mod timer_manager;
mod utils;

/// A more readable placeholder for a stable Never (`!`) type.
///
/// It is recommended to use this in port directions and actor types, which do not expect any messages, instead of the unit type `()`.
/// This way the compiler should correctly identify any handlers enforced to be implemented by the API as dead code and eliminate them, resulting in smaller code sizes.
#[cfg(nightly)]
pub type Never = !;

/// A more readable placeholder for a stable Never (`!`) type.
///
/// Only nightly this defaults to `!` and will eventually be replaced with that once `never_type` stabilises.
///
/// It is recommended to use this in port directions and actor types, which do not expect any messages, instead of the unit type `()`.
/// This way the compiler should correctly identify any handlers enforced to be implemented by the API as dead code and eliminate them, resulting in smaller code sizes.
#[cfg(not(nightly))]
#[derive(Debug, Clone, PartialEq, Eq, Copy)]
pub enum Never {}

/// To get all kompact related things into scope import as `use kompact::prelude::*`.
pub mod prelude {
    pub use slog::{crit, debug, error, info, o, trace, warn, Drain, Fuse, Logger};

    pub use std::{
        any::Any,
        convert::{From, Into},
    };

    pub use bytes::{Buf, BufMut, IntoBuf};

    pub use kompact_actor_derive::*;
    pub use kompact_component_derive::*;

    pub use crate::{ignore_control, match_deser};

    pub use crate::{
        actors::{
            Actor,
            ActorPath,
            ActorPathFactory,
            ActorRaw,
            ActorRef,
            ActorRefFactory,
            ActorRefStrong,
            ActorSource,
            Dispatcher,
            DispatcherRef,
            Dispatching,
            DispatchingPath,
            MessageBounds,
            NamedPath,
            NetworkActor,
            Receiver,
            Recipient,
            Request,
            SystemPath,
            Transport,
            UniquePath,
            WithRecipient,
            WithSender,
            WithSenderStrong,
        },
        component::{
            Component,
            ComponentContext,
            ComponentDefinition,
            ComponentLogging,
            CoreContainer,
            ExecuteResult,
            LockingProvideRef,
            LockingRequireRef,
            Provide,
            ProvideRef,
            Require,
            RequireRef,
        },
        lifecycle::{ControlEvent, ControlPort},
        ports::{Port, ProvidedPort, ProvidedRef, RequiredPort, RequiredRef},
        runtime::{KompactConfig, KompactSystem, SystemHandle},
        Never,
    };

    pub use crate::{
        default_components::{CustomComponents, DeadletterBox},
        dispatch::{NetworkConfig, NetworkDispatcher},
        messaging::{
            DispatchEnvelope,
            MsgEnvelope,
            NetMessage,
            PathResolvable,
            RegistrationError,
            Serialised,
            UnpackError,
        },
        timer_manager::{ScheduledTimer, Timer, TimerRefFactory},
    };

    pub use crate::{
        serialisation::*,
        utils::{
            biconnect_components,
            biconnect_ports,
            on_dual_definition,
            promise as kpromise,
            Ask,
            Fulfillable,
            Future as KFuture,
            IterExtras,
            Promise as KPromise,
            PromiseErr,
            TryDualLockError,
        },
    };
}

/// A module containing helper functions for (unit) testing
///
/// Import all with `use prelude_test::*;`.
pub mod prelude_test {
    pub use crate::serialisation::ser_test_helpers;
}

/// Helper structs and functions for doctests.
///
/// Please simply ignore this module, which should be gated by `#[cfg(doctest)]`,
/// which doesn't seem to [work properly](https://github.com/rust-lang/rust/issues/67295).
pub mod doctest_helpers {
    use crate::prelude::*;

    /// A quick test path to create an [ActorPath](ActorPath) with
    pub const TEST_PATH: &'static str = "local://127.0.0.1:0/test_actor";

    /// A test port
    pub struct TestPort;
    impl Port for TestPort {
        type Indication = Never;
        type Request = Never;
    }

    /// A test component
    #[derive(ComponentDefinition, Actor)]
    pub struct TestComponent1 {
        ctx: ComponentContext<Self>,
        test_port: ProvidedPort<TestPort, Self>,
    }
    impl TestComponent1 {
        /// Create a new test component
        pub fn new() -> TestComponent1 {
            TestComponent1 {
                ctx: ComponentContext::new(),
                test_port: ProvidedPort::new(),
            }
        }
    }
    ignore_control!(TestComponent1);
    impl Provide<TestPort> for TestComponent1 {
        fn handle(&mut self, _event: Never) -> () {
            unreachable!();
        }
    }

    /// Another test component
    #[derive(ComponentDefinition, Actor)]
    pub struct TestComponent2 {
        ctx: ComponentContext<Self>,
        test_port: RequiredPort<TestPort, Self>,
    }
    impl TestComponent2 {
        /// Create a new test component
        pub fn new() -> TestComponent2 {
            TestComponent2 {
                ctx: ComponentContext::new(),
                test_port: RequiredPort::new(),
            }
        }
    }
    ignore_control!(TestComponent2);
    impl Require<TestPort> for TestComponent2 {
        fn handle(&mut self, _event: Never) -> () {
            unreachable!();
        }
    }
}

/// A simple type alias Kompact's slog `Logger` type signature.
pub type KompactLogger = Logger<std::sync::Arc<Fuse<Async>>>;

#[cfg(test)]
mod test_helpers {
    use std::{
        env,
        fs,
        ops::Deref,
        path::{Path, PathBuf},
    };
    use tempfile::TempDir;

    // liberally borrowed from https://andrewra.dev/2019/03/01/testing-in-rust-temporary-files/
    pub struct Fixture {
        path: PathBuf,
        source: PathBuf,
        _tempdir: TempDir,
    }

    impl Fixture {
        #[allow(unused)]
        pub fn blank(fixture_filename: &str) -> Self {
            // First, figure out the right file in `tests/fixtures/`:
            let root_dir = &env::var("CARGO_MANIFEST_DIR").expect("$CARGO_MANIFEST_DIR");
            let mut source = PathBuf::from(root_dir);
            source.push("tests/fixtures");
            source.push(&fixture_filename);

            // The "real" path of the file is going to be under a temporary directory:
            let tempdir = tempfile::tempdir().unwrap();
            let mut path = PathBuf::from(&tempdir.path());
            path.push(&fixture_filename);

            Fixture {
                _tempdir: tempdir,
                source,
                path,
            }
        }

        #[allow(unused)]
        pub fn copy(fixture_filename: &str) -> Self {
            let fixture = Fixture::blank(fixture_filename);
            fs::copy(&fixture.source, &fixture.path).unwrap();
            fixture
        }
    }

    impl Deref for Fixture {
        type Target = Path;

        fn deref(&self) -> &Self::Target {
            self.path.deref()
        }
    }

    impl AsRef<Path> for Fixture {
        fn as_ref(&self) -> &Path {
            self.path.as_ref()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_helpers::*;

    use std::{fs::File, io::Write, ops::Deref, thread, time};
    //use futures::{Future, future};
    //use futures_cpupool::CpuPool;
    use super::prelude::*;
    use std::{sync::Arc, time::Duration};

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

    impl Actor for RecvComponent {
        type Message = &'static str;

        fn receive_local(&mut self, msg: Self::Message) -> () {
            info!(self.ctx.log(), "RecvComponent received {:?}", msg);
            self.last_string = msg.to_string();
        }

        fn receive_network(&mut self, msg: NetMessage) -> () {
            error!(self.ctx.log(), "Got unexpected network message: {:?}", msg);
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
        let system = settings.build().expect("KompactSystem");
        test_with_system(system);
    }

    fn test_with_system(system: KompactSystem) -> () {
        let tc = system.create(TestComponent::new);
        let rc = system.create(RecvComponent::new);
        let rctp: RequiredRef<TestPort> = rc.required_ref();
        let tctp: ProvidedRef<TestPort> = tc.on_definition(|c| {
            c.test_port.connect(rctp);
            c.provided_ref()
        });
        system.start(&tc);
        system.start(&rc);
        let msg = Arc::new(1234);
        system.trigger_r(msg, &tctp);

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
        rcref.tell("MsgTest");

        thread::sleep(ten_millis);

        rc.on_definition(|c| {
            //println!("Last string was {}", c.last_string);
            assert_eq!(c.last_string, String::from("MsgTest"));
        });

        system
            .shutdown()
            .expect("Kompact didn't shut down properly");
    }
    #[derive(ComponentDefinition)]
    struct DedicatedComponent {
        ctx: ComponentContext<Self>,
        target: ActorRef<Box<dyn Any + Send>>,
    }

    impl DedicatedComponent {
        fn new(target: ActorRef<Box<dyn Any + Send>>) -> Self {
            DedicatedComponent {
                ctx: ComponentContext::new(),
                target,
            }
        }
    }

    impl Provide<ControlPort> for DedicatedComponent {
        fn handle(&mut self, event: ControlEvent) -> () {
            match event {
                ControlEvent::Start => {
                    info!(self.ctx.log(), "Starting DedicatedComponent");
                }
                ControlEvent::Stop => {
                    info!(self.ctx.log(), "Stopping DedicatedComponent");
                }
                _ => (), // ignore
            }
        }
    }

    impl Actor for DedicatedComponent {
        type Message = String;

        fn receive_local(&mut self, _msg: Self::Message) -> () {
            self.target
                .tell(Box::new(String::from("hello")) as Box<dyn Any + Send>);
        }

        fn receive_network(&mut self, msg: NetMessage) -> () {
            crit!(self.ctx.log(), "Got unexpected message {:?}", msg);
            unimplemented!(); // shouldn't happen during the test
        }
    }

    #[test]
    fn test_dedicated_ref() -> () {
        let system = KompactConfig::default().build().expect("System");
        let cc = system.create_dedicated(CounterComponent::new);
        system.start(&cc);
        let cc_ref: ActorRef<Box<dyn Any + Send>> = cc.actor_ref();
        let dc = system.create_dedicated(move || DedicatedComponent::new(cc_ref));
        system.start(&dc);

        let thousand_millis = time::Duration::from_millis(1000);
        thread::sleep(thousand_millis);

        let dc_ref: ActorRef<String> = dc.actor_ref();
        dc_ref.tell(String::from("go"));

        thread::sleep(thousand_millis);

        cc.on_definition(|c| {
            assert_eq!(c.msg_count, 1);
        });

        system
            .shutdown()
            .expect("Kompact didn't shut down properly");
    }

    #[test]
    #[cfg(feature = "thread_pinning")]
    fn test_dedicated_pinning() -> () {
        let core_ids = core_affinity::get_core_ids().expect("Failed to fetch core ids");
        if core_ids.len() < 2 {
            panic!("this test requires at least two cores");
        }

        let system = KompactConfig::default().build().expect("System");
        let cc = system.create_dedicated_pinned(CounterComponent::new, core_ids[0]);
        system.start(&cc);
        let cc_ref: ActorRef<Box<dyn Any + Send>> = cc.actor_ref();
        let dc =
            system.create_dedicated_pinned(move || DedicatedComponent::new(cc_ref), core_ids[1]);
        system.start(&dc);

        let thousand_millis = time::Duration::from_millis(1000);
        thread::sleep(thousand_millis);

        let dc_ref: ActorRef<String> = dc.actor_ref();
        dc_ref.tell(String::from("go"));

        thread::sleep(thousand_millis);

        cc.on_definition(|c| {
            assert_eq!(c.msg_count, 1);
        });

        system
            .shutdown()
            .expect("Kompact didn't shut down properly");
    }

    #[test]
    fn test_dedicated() -> () {
        let system = KompactConfig::default().build().expect("System");

        let tc = system.create_dedicated(TestComponent::new);
        let rc = system.create_dedicated(RecvComponent::new);
        let rctp: RequiredRef<TestPort> = rc.required_ref();
        let tctp: ProvidedRef<TestPort> = tc.on_definition(|c| {
            c.test_port.connect(rctp);
            c.provided_ref()
        });
        system.start(&tc);
        system.start(&rc);
        let msg = Arc::new(1234);
        system.trigger_r(msg, &tctp);

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
        rcref.tell("MsgTest");

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
        let trc = system.create(TimerRecvComponent::new);
        system.start(&trc);

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
        type Message = Box<dyn Any + Send>;

        fn receive_local(&mut self, _msg: Self::Message) -> () {
            info!(self.ctx.log(), "CounterComponent got a message!");
            self.msg_count += 1;
        }

        fn receive_network(&mut self, msg: NetMessage) -> () {
            crit!(self.ctx.log(), "Got unexpected message {:?}", msg);
            unimplemented!(); // shouldn't happen during the test
        }
    }

    #[test]
    fn test_start_stop() -> () {
        let system = KompactConfig::default().build().expect("KompactSystem");
        let (cc, _) = system.create_and_register(CounterComponent::new);
        let ccref = cc.actor_ref();

        ccref.tell(Box::new(String::from("MsgTest")) as Box<dyn Any + Send>);

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
        ccref.tell(Box::new(String::from("MsgTest")) as Box<dyn Any + Send>);

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
        type Message = Box<dyn Any + Send>;

        fn receive_local(&mut self, _msg: Self::Message) -> () {
            info!(self.ctx.log(), "Crashing CrasherComponent");
            panic!("Test panic please ignore");
        }

        fn receive_network(&mut self, _msg: NetMessage) -> () {
            info!(self.ctx.log(), "Crashing CrasherComponent");
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

            let crash_port: ProvidedRef<CrashPort> = cc.provided_ref();

            let f = system.start_notify(&cc);

            let res = f.wait_timeout(Duration::from_millis(1000));
            assert!(res.is_ok(), "Component should not crash on start");

            system.trigger_r((), &crash_port);

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

            ccref.tell(Box::new(()) as Box<dyn Any + Send>);

            thread::sleep(Duration::from_millis(1000));

            assert!(cc.is_faulty(), "Component should have crashed.");
        }

        system
            .shutdown()
            .expect("Kompact didn't shut down properly");
    }

    #[derive(ComponentDefinition, Actor)]
    struct Stopper {
        ctx: ComponentContext<Self>,
    }
    impl Stopper {
        fn new() -> Stopper {
            Stopper {
                ctx: ComponentContext::new(),
            }
        }
    }
    impl Provide<ControlPort> for Stopper {
        fn handle(&mut self, event: ControlEvent) -> () {
            match event {
                ControlEvent::Start => {
                    self.ctx().system().shutdown_async();
                }
                _ => (), // ignore
            }
        }
    }

    #[test]
    fn test_async_shutdown() -> () {
        let system = KompactConfig::default().build().expect("system");
        let stopper = system.create(Stopper::new);
        system.start(&stopper);
        system.await_termination();
    }

    #[derive(ComponentDefinition, Actor)]
    struct ConfigComponent {
        ctx: ComponentContext<Self>,
        test_value: Option<i64>,
    }
    impl ConfigComponent {
        fn new() -> ConfigComponent {
            ConfigComponent {
                ctx: ComponentContext::new(),
                test_value: None,
            }
        }
    }
    impl Provide<ControlPort> for ConfigComponent {
        fn handle(&mut self, event: ControlEvent) -> () {
            match event {
                ControlEvent::Start => {
                    self.test_value = self.ctx().config()["a"].as_i64();
                    self.ctx().system().shutdown_async();
                }
                _ => (), // ignore
            }
        }
    }

    #[test]
    fn test_config_from_string() -> () {
        let default_values = r#"{ a = 7 }"#;
        let mut conf = KompactConfig::default();
        conf.load_config_str(default_values);
        let system = conf.build().expect("system");
        let c = system.create(ConfigComponent::new);
        system.start(&c);
        system.await_termination();
        c.on_definition(|cd| {
            assert_eq!(7i64, cd.test_value.take().unwrap());
        });
    }

    #[test]
    fn test_config_from_file() -> () {
        //let default_values = r#"{ a = 7 }"#;
        let config_file_path = Fixture::blank("application.conf");
        let mut config_file = File::create(config_file_path.deref()).expect("config file");
        config_file
            .write_all(b"{ a = 7 }")
            .expect("write config file");
        let mut conf = KompactConfig::default();
        conf.load_config_file(config_file_path.to_path_buf());
        let system = conf.build().expect("system");
        let c = system.create(ConfigComponent::new);
        system.start(&c);
        system.await_termination();
        c.on_definition(|cd| {
            assert_eq!(7i64, cd.test_value.take().unwrap());
        });
    }

    #[test]
    fn test_config_merged() -> () {
        let default_values = r#"{ a = 5 }"#;
        let config_file_path = Fixture::blank("application.conf");
        let mut config_file = File::create(config_file_path.deref()).expect("config file");
        config_file
            .write_all(b"{ a = 7 }")
            .expect("write config file");
        let mut conf = KompactConfig::default();
        conf.load_config_str(default_values)
            .load_config_file(config_file_path.to_path_buf());
        let system = conf.build().expect("system");
        let c = system.create(ConfigComponent::new);
        system.start(&c);
        system.await_termination();
        c.on_definition(|cd| {
            assert_eq!(7i64, cd.test_value.take().unwrap());
        });
    }
}

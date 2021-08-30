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
//!             ctx: ComponentContext::uninitialised()
//!         }
//!     }
//! }
//! impl ComponentLifecycle for HelloWorldComponent {
//!     fn on_start(&mut self) -> Handled {
//!         info!(self.ctx.log(), "Hello World!");
//!         self.ctx().system().shutdown_async();
//!         Handled::Ok
//!     }
//! }
//!
//! let system = KompactConfig::default().build().expect("system");
//! let component = system.create(HelloWorldComponent::new);
//! system.start(&component);
//! system.await_termination();
//! ```
//!
//! # Crate Feature Flags
//!
//! The following crate feature flags are available. They are configured in your Cargo.toml.
//!
//! - `silent_logging`
//!     - Reduces logging to INFO in debug builds and ERROR in release builds
//!     - Must be used with `--no-default-features`
//! - `low_latency`
//!     - Prevents thread pool threads from going to sleep when using the default pool.
//!     - This can improve reaction time to incoming network events, at the cost of much higher CPU utilisation.
//! - `ser_id_64` (default), `ser_id_32`, `ser_id_16`, `ser_id_8`
//!     - Selects the bit-width of serialisation identifiers.
//!     - Trying to use a serialisation id that is too large for the selected width will result in a compile-time error.
//!     - All values are mutually exclusive and all but the default of `ser_id_64` must be used with `--no-default-features`.
//! - `thread_pinning`
//!     - Enables support for pinning pool threads to CPU cores (or rather processing units).
//!     - This flag has no effect if the runtime OS does not support thread pinning.
//!     - Enabling this will cause as many threads as there are PUs to be pinned by default.
//!     - Assignments can be customised by providing a custom scheduler with the desired settings.
//!     - See [executors crate](https://docs.rs/executors/0.8.0/executors/crossbeam_workstealing_pool/struct.ThreadPool.html?search=#method.with_affinity) for more details.
//! - `serde_support` (default)
//!     - Build with support for [Serde](https://github.com/serde-rs/serde) serialisers.
//! - `type_erasure` (nightly-only)
//!     - Build with an experimental API for `dyn` type-erased components.
//! - `use_local_executor` (default)
//!     - Use thread-local executors to avoid cloning the handle to the system scheduler for every component scheduling.
//!     - Not all scheduler implementations support this feature, so you might have to disable it via `--no-default-features` if you are using a custom scheduler.
//! - `implicit_routes` (default)
//!     - Allow default broadcast and select actor paths on any node in the tree, not just where explicitly set via [set_routing_policy](KompactSystem::set_routing_policy).
//!     - While this feature is convenient, it may open up your system to DoS attacks via broadcast on high-level nodes (e.g. `tcp://1.2.3.4:8000/*`).
//!     - If you are concered about this security risk, you can disable this feature by using `--no-default-features`.

#![deny(missing_docs)]
#![allow(clippy::unused_unit)]
#![allow(clippy::match_ref_pats)]
#![allow(clippy::new_without_default)]
#![cfg_attr(nightly, feature(never_type))]
#![cfg_attr(nightly, feature(async_closure))]
#![cfg_attr(nightly, feature(unsized_fn_params))] // requires nightly > 2020-10-29

#[cfg(feature = "thread_pinning")]
pub use core_affinity::{get_core_ids, CoreId};

#[cfg(feature = "protobuf")]
pub use self::serialisation::protobuf_serialisers;
#[cfg(feature = "serde_support")]
pub use self::serialisation::serde_serialisers;
use self::{
    actors::*,
    component::*,
    default_components::*,
    lifecycle::*,
    ports::*,
    runtime::*,
    serialisation::*,
    utils::*,
};
use crossbeam_queue::SegQueue as ConcurrentQueue;
/// The default crate for scheduler implementations
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
/// Utilities for working with Kompact configuration keys and values
#[macro_use]
pub mod config;
mod dedicated_scheduler;
/// Default implementations for system components
pub mod default_components;
mod dispatch;
/// Facilities and utilities for dealing with network messages
pub mod messaging;
/// Default networking implementation
pub mod net;
mod ports;
/// Facilities for routing messages
pub mod routing;
/// Kompact system runtime facilities, such as configuration and schedulers
pub mod runtime;
mod serialisation;
mod supervision;
/// Reusable timer facility internals
pub mod timer;
mod utils;

pub use dispatch::lookup;

/// A more readable placeholder for a stable Never (`!`) type.
///
/// It is recommended to use this in port directions and actor types, which do not expect any messages, instead of the unit type `()`.
/// This way the compiler should correctly identify any handlers enforced to be implemented by the API as dead code and eliminate them, resulting in smaller code sizes.
#[cfg(nightly)]
pub type Never = !;

/// A more readable placeholder for a stable Never (`!`) type.
///
/// On nightly this defaults to `!` and will eventually be replaced with that once `never_type` stabilises.
///
/// It is recommended to use this in port directions and actor types, which do not expect any messages, instead of the unit type `()`.
/// This way the compiler should correctly identify any handlers enforced to be implemented by the API as dead code and eliminate them, resulting in smaller code sizes.
#[cfg(not(nightly))]
pub type Never = std::convert::Infallible;

/// A type of future returned from the [spawn](KompactSystem::spawn) and [spawn_off](ComponentDefinition::spawn_off) functions to await
/// the completion of the spawned future.
///
/// This API currently does not support cancellation,
/// but that feature may be added in a future API if needed.
pub type JoinHandle<R> = futures::channel::oneshot::Receiver<R>;

/// A simple type alias Kompact's slog `Logger` type signature.
pub type KompactLogger = Logger<std::sync::Arc<Fuse<Async>>>;

/// Useful Kompact constants are re-exported in this module
pub mod constants {
    pub use crate::actors::{BROADCAST_MARKER, PATH_SEP, SELECT_MARKER, UNIQUE_PATH_SEP};
}

/// All Kompact configuration keys
pub mod config_keys {
    pub use crate::runtime::keys as system;
}

/// To get all kompact related things into scope import as `use kompact::prelude::*`.
pub mod prelude {
    pub use slog::{crit, debug, error, info, o, trace, warn, Drain, Fuse, Logger};

    pub use bytes::{Buf, BufMut};
    pub use std::{
        any::Any,
        convert::{From, Into},
    }; // IntoBuf

    pub use kompact_actor_derive::*;
    pub use kompact_component_derive::*;

    #[allow(deprecated)]
    pub use crate::{
        ignore_control,
        ignore_indications,
        ignore_lifecycle,
        ignore_requests,
        info_lifecycle,
        match_deser,
    };

    pub use crate::{
        actors::{
            Actor,
            ActorPath,
            ActorPathFactory,
            ActorRaw,
            ActorRef,
            ActorRefFactory,
            ActorRefStrong,
            Dispatcher,
            DispatcherRef,
            Dispatching,
            DispatchingPath,
            DynActorRef,
            DynActorRefFactory,
            MessageBounds,
            NamedPath,
            NetworkActor,
            PathParseError,
            Receiver,
            Recipient,
            Request,
            SystemField,
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
            ComponentDefinitionAccess,
            ComponentLifecycle,
            ComponentLogging,
            CoreContainer,
            DynamicPortAccess,
            ExecuteResult,
            Handled,
            LockingProvideRef,
            LockingRequireRef,
            Provide,
            ProvideRef,
            Require,
            RequireRef,
        },
        net::{
            buffers::{BufferConfig, ChunkLease, ChunkRef},
            SessionId,
        },
        ports::{
            Channel,
            DisconnectError,
            Port,
            ProvidedPort,
            ProvidedRef,
            ProviderChannel,
            RequiredPort,
            RequiredRef,
            RequirerChannel,
            TryLockError,
            TwoWayChannel,
        },
        runtime::{KompactConfig, KompactSystem, SystemHandle},
        supervision::{FaultContext, RecoveryHandler},
        Never,
    };

    pub use crate::{
        default_components::{CustomComponents, DeadletterBox, LocalDispatcher},
        dispatch::{
            NetworkConfig,
            NetworkDispatcher,
            NetworkStatus,
            NetworkStatusPort,
            NetworkStatusRequest,
        },
        messaging::{
            DispatchEnvelope,
            MsgEnvelope,
            NetMessage,
            PathResolvable,
            RegistrationError,
            RegistrationResult,
            Serialised,
            UnpackError,
        },
        timer::timer_manager::{CanCancelTimers, ScheduledTimer, Timer, TimerRefFactory},
    };

    pub use crate::{
        serialisation::*,
        utils::{
            biconnect_components,
            biconnect_ports,
            block_on,
            block_until,
            on_dual_definition,
            promise,
            Ask,
            Completable,
            Fulfillable,
            FutureCollection,
            FutureResultCollection,
            IterExtras,
            KFuture,
            KPromise,
            PromiseErr,
            TryDualLockError,
        },
    };

    pub use crate::routing::groups::StorePolicy;

    #[cfg(all(nightly, feature = "type_erasure"))]
    pub use crate::utils::erased::CreateErased;
}

/// A module containing helper functions for (unit) testing
///
/// Import all with `use prelude_test::*;`.
pub mod prelude_test {
    pub use crate::{net::net_test_helpers, serialisation::ser_test_helpers};
}

/// A module containing helper functions for benchmarking
pub mod prelude_bench {
    pub use crate::actors::{parse_path, validate_insert_path, validate_lookup_path};
}

/// Helper structs and functions for doctests.
///
/// Please simply ignore this module, which should be gated by `#[cfg(doctest)]`,
/// which doesn't seem to [work properly](https://github.com/rust-lang/rust/issues/67295).
pub mod doctest_helpers {
    use crate::prelude::*;

    /// A quick test path to create an [ActorPath](ActorPath) with
    pub const TEST_PATH: &str = "local://127.0.0.1:0/test_actor";

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
        test_port: ProvidedPort<TestPort>,
    }

    impl TestComponent1 {
        /// Create a new test component
        pub fn new() -> TestComponent1 {
            TestComponent1 {
                ctx: ComponentContext::uninitialised(),
                test_port: ProvidedPort::uninitialised(),
            }
        }
    }
    ignore_lifecycle!(TestComponent1);
    impl Provide<TestPort> for TestComponent1 {
        fn handle(&mut self, _event: Never) -> Handled {
            unreachable!();
        }
    }

    /// Another test component
    #[derive(ComponentDefinition, Actor)]
    pub struct TestComponent2 {
        ctx: ComponentContext<Self>,
        test_port: RequiredPort<TestPort>,
    }

    impl TestComponent2 {
        /// Create a new test component
        pub fn new() -> TestComponent2 {
            TestComponent2 {
                ctx: ComponentContext::uninitialised(),
                test_port: RequiredPort::uninitialised(),
            }
        }
    }
    ignore_lifecycle!(TestComponent2);
    impl Require<TestPort> for TestComponent2 {
        fn handle(&mut self, _event: Never) -> Handled {
            unreachable!();
        }
    }
}

#[cfg(test)]
mod test_helpers {
    use std::{
        env,
        fs,
        ops::Deref,
        path::{Path, PathBuf},
    };
    use tempfile::TempDir;

    /// A quick test path to create an [ActorPath](ActorPath) with
    pub const TEST_PATH: &str = "local://127.0.0.1:0/test_actor";

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
    use std::sync::Mutex;

    use super::prelude::*;
    use std::{fs::File, io::Write, ops::Deref, sync::Arc, thread, time, time::Duration};

    use once_cell::sync::Lazy;

    struct TestPort;

    impl Port for TestPort {
        type Indication = Arc<String>;
        type Request = Arc<u64>;
    }

    #[derive(ComponentDefinition, Actor)]
    struct TestComponent {
        ctx: ComponentContext<TestComponent>,
        test_port: ProvidedPort<TestPort>,
        counter: u64,
    }

    impl TestComponent {
        fn new() -> TestComponent {
            TestComponent {
                ctx: ComponentContext::uninitialised(),
                counter: 0,
                test_port: ProvidedPort::uninitialised(),
            }
        }
    }

    ignore_lifecycle!(TestComponent);

    impl Provide<TestPort> for TestComponent {
        fn handle(&mut self, event: Arc<u64>) -> Handled {
            self.counter += *event;
            self.test_port.trigger(Arc::new(String::from("Test")));
            Handled::Ok
        }
    }

    #[derive(ComponentDefinition)]
    struct RecvComponent {
        ctx: ComponentContext<RecvComponent>,
        test_port: RequiredPort<TestPort>,
        last_string: String,
    }

    impl RecvComponent {
        fn new() -> RecvComponent {
            RecvComponent {
                ctx: ComponentContext::uninitialised(),
                test_port: RequiredPort::uninitialised(),
                last_string: String::from("none ;("),
            }
        }
    }

    impl Actor for RecvComponent {
        type Message = &'static str;

        fn receive_local(&mut self, msg: Self::Message) -> Handled {
            info!(self.ctx.log(), "RecvComponent received {:?}", msg);
            self.last_string = msg.to_string();
            Handled::Ok
        }

        fn receive_network(&mut self, msg: NetMessage) -> Handled {
            error!(self.ctx.log(), "Got unexpected network message: {:?}", msg);
            unimplemented!(); // shouldn't happen during the test
        }
    }

    ignore_lifecycle!(RecvComponent);

    impl Require<TestPort> for RecvComponent {
        fn handle(&mut self, event: Arc<String>) -> Handled {
            info!(self.ctx.log(), "Got event {}", event.as_ref());
            self.last_string = event.as_ref().clone();
            Handled::Ok
        }
    }

    #[test]
    fn default_settings() {
        let system = KompactConfig::default().build().expect("KompactSystem");

        test_with_system(system);
    }

    #[test]
    fn custom_settings() {
        let mut settings = KompactConfig::default();
        settings.set_config_value(
            &crate::config_keys::system::LABEL,
            "custom-system".to_string(),
        );
        settings.set_config_value(&crate::config_keys::system::THROUGHPUT, 10);
        settings.set_config_value(&crate::config_keys::system::MESSAGE_PRIORITY, 0.1);
        let system = settings.build().expect("KompactSystem");
        assert_eq!(10, system.throughput());
        assert_eq!(1, system.max_messages());
        test_with_system(system);
    }

    #[test]
    fn custom_executor() {
        let mut settings = KompactConfig::default();
        settings
            .set_config_value(&crate::config_keys::system::THREADS, 2)
            .executor(executors::crossbeam_channel_pool::ThreadPool::new);
        let system = settings.build().expect("KompactSystem");
        test_with_system(system);
    }

    #[test]
    fn custom_scheduler() {
        let mut settings = KompactConfig::default();
        settings.scheduler(move |t| {
            crate::runtime::ExecutorScheduler::from(
                executors::crossbeam_channel_pool::ThreadPool::new(t),
            )
        });
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
                ctx: ComponentContext::uninitialised(),
                target,
            }
        }
    }

    ignore_lifecycle!(DedicatedComponent);

    impl Actor for DedicatedComponent {
        type Message = String;

        fn receive_local(&mut self, _msg: Self::Message) -> Handled {
            self.target
                .tell(Box::new(String::from("hello")) as Box<dyn Any + Send>);
            Handled::Ok
        }

        fn receive_network(&mut self, msg: NetMessage) -> Handled {
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
                ctx: ComponentContext::uninitialised(),
                last_string: String::from("none ;("),
            }
        }
    }

    impl ComponentLifecycle for TimerRecvComponent {
        fn on_start(&mut self) -> Handled {
            info!(self.ctx.log(), "Starting TimerRecvComponent");
            self.schedule_once(Duration::from_millis(100), |self_c, _| {
                self_c.last_string = String::from("TimerTest");
                Handled::Ok
            });

            Handled::Ok
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
                ctx: ComponentContext::uninitialised(),
                msg_count: 0,
            }
        }
    }

    ignore_lifecycle!(CounterComponent);

    impl Actor for CounterComponent {
        type Message = Box<dyn Any + Send>;

        fn receive_local(&mut self, _msg: Self::Message) -> Handled {
            info!(self.ctx.log(), "CounterComponent got a message!");
            self.msg_count += 1;
            Handled::Ok
        }

        fn receive_network(&mut self, msg: NetMessage) -> Handled {
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
        crash_port: ProvidedPort<CrashPort>,
        crash_on_start: bool,
    }

    impl CrasherComponent {
        fn new(crash_on_start: bool) -> CrasherComponent {
            CrasherComponent {
                ctx: ComponentContext::uninitialised(),
                crash_port: ProvidedPort::uninitialised(),
                crash_on_start,
            }
        }
    }

    impl ComponentLifecycle for CrasherComponent {
        fn on_start(&mut self) -> Handled {
            if self.crash_on_start {
                info!(self.ctx.log(), "Crashing CrasherComponent");
                panic!("Test panic please ignore");
            } else {
                info!(self.ctx.log(), "Starting CrasherComponent");
                Handled::Ok
            }
        }
    }

    impl Provide<CrashPort> for CrasherComponent {
        fn handle(&mut self, _event: ()) -> Handled {
            info!(self.ctx.log(), "Crashing CounterComponent");
            panic!("Test panic please ignore");
        }
    }

    impl Actor for CrasherComponent {
        type Message = Box<dyn Any + Send>;

        fn receive_local(&mut self, _msg: Self::Message) -> Handled {
            info!(self.ctx.log(), "Crashing CrasherComponent");
            panic!("Test panic please ignore");
        }

        fn receive_network(&mut self, _msg: NetMessage) -> Handled {
            info!(self.ctx.log(), "Crashing CrasherComponent");
            panic!("Test panic please ignore");
        }
    }

    // replace ignore with panic cfg gate when https://github.com/rust-lang/rust/pull/74754 is merged
    #[test]
    #[ignore]
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

    // replace ignore with panic cfg gate when https://github.com/rust-lang/rust/pull/74754 is merged
    #[test]
    #[ignore]
    fn test_component_recovery() -> () {
        let system = KompactConfig::default().build().expect("KompactSystem");

        let (sender, receiver) = std::sync::mpsc::channel::<Arc<Component<RecvComponent>>>();

        // limit scope
        let cc = system.create(|| CrasherComponent::new(false));

        cc.set_recovery_function(move |fault| {
            fault.recover_with(move |_ctx, system, log| {
                info!(log, "Recovering into RecvComponent");
                let rcd = system.create(RecvComponent::new);
                system.start(&rcd);
                sender.send(rcd).expect("sent component");
            })
        });

        let ccref = cc.actor_ref();

        let f = system.start_notify(&cc);

        let res = f.wait_timeout(Duration::from_millis(1000));
        assert!(res.is_ok(), "Component should not crash on start");

        ccref.tell(Box::new(()) as Box<dyn Any + Send>);

        let rcd = receiver
            .recv_timeout(Duration::from_millis(1000))
            .expect("receive component");

        assert!(cc.is_faulty(), "Component should have crashed.");

        let rcd_ref = rcd.actor_ref();
        rcd_ref.tell("MsgTest");

        thread::sleep(Duration::from_millis(1000));

        rcd.on_definition(|c| {
            assert_eq!(c.last_string, String::from("MsgTest"));
        });

        system
            .shutdown()
            .expect("Kompact didn't shut down properly");
    }

    #[derive(Debug)]
    enum StringMsg {
        Get(KPromise<&'static str>),
        Put(&'static str),
        Crash,
    }

    #[derive(ComponentDefinition)]
    struct RecovererComponent {
        ctx: ComponentContext<Self>,
        last_string: &'static str,
    }

    static RECOVERER_CURRENT_REF: Lazy<Mutex<Option<ActorRef<StringMsg>>>> =
        Lazy::new(|| Mutex::new(None));

    impl RecovererComponent {
        fn set_current_ref(aref: ActorRef<<Self as Actor>::Message>) -> () {
            let mut guard = RECOVERER_CURRENT_REF.lock().expect("ref guard");
            *guard = Some(aref);
        }

        fn get_current_ref() -> Option<ActorRef<StringMsg>> {
            let guard = RECOVERER_CURRENT_REF.lock().expect("ref guard");
            guard.clone()
        }

        #[allow(dead_code)]
        fn unset_current_ref() -> () {
            let mut guard = RECOVERER_CURRENT_REF.lock().expect("ref guard");
            *guard = None;
        }

        fn new() -> Self {
            RecovererComponent {
                ctx: ComponentContext::uninitialised(),
                last_string: "created",
            }
        }

        fn with_msg(msg: &'static str) -> Self {
            RecovererComponent {
                ctx: ComponentContext::uninitialised(),
                last_string: msg,
            }
        }
    }

    impl Default for RecovererComponent {
        fn default() -> Self {
            RecovererComponent {
                ctx: ComponentContext::uninitialised(),
                last_string: "default",
            }
        }
    }

    impl ComponentLifecycle for RecovererComponent {
        fn on_start(&mut self) -> Handled {
            info!(self.ctx.log(), "Starting RecovererComponent");
            let aref = self.actor_ref();
            Self::set_current_ref(aref);
            self.ctx
                .set_recovery_function(move |fault| fault.restart_default::<RecovererComponent>());
            Handled::Ok
        }
    }

    impl Actor for RecovererComponent {
        type Message = StringMsg;

        fn receive_local(&mut self, msg: Self::Message) -> Handled {
            info!(self.ctx.log(), "Got msg: {:?}", msg);
            match msg {
                StringMsg::Get(promise) => {
                    promise.fulfil(self.last_string).expect("fulfilled");
                }
                StringMsg::Put(s) => {
                    self.last_string = s;
                    self.ctx.set_recovery_function(move |fault| {
                        fault.recover_with(move |_ctx, system, logger| {
                            info!(logger, "Recovering with message: {}", s);
                            let rc = system.create(move || RecovererComponent::with_msg(s));
                            let rc_ref = rc.actor_ref();
                            Self::set_current_ref(rc_ref);
                            system.start(&rc);
                        })
                    });
                }
                StringMsg::Crash => {
                    info!(self.ctx.log(), "Crashing RecovererComponent");
                    panic!("Test panic please ignore");
                }
            }
            Handled::Ok
        }

        fn receive_network(&mut self, _msg: NetMessage) -> Handled {
            unimplemented!();
        }
    }

    // replace ignore with panic cfg gate when https://github.com/rust-lang/rust/pull/74754 is merged
    #[test]
    #[ignore]
    fn test_component_recovery_with_state() -> () {
        let system = KompactConfig::default().build().expect("KompactSystem");

        let one_sec = Duration::from_millis(1000);

        // limit scope
        let rc = system.create(RecovererComponent::new);

        let f = system.start_notify(&rc);

        let res = f.wait_timeout(one_sec);

        assert!(res.is_ok(), "Component should not crash on start");

        let rc_ref = RecovererComponent::get_current_ref().expect("ref");

        rc_ref.tell(StringMsg::Crash);

        thread::sleep(one_sec);

        assert!(rc.is_faulty(), "Component should have crashed.");

        // get new reference
        let rc_ref = RecovererComponent::get_current_ref().expect("ref");

        let (p, f) = promise::<&'static str>();

        rc_ref.tell(StringMsg::Get(p));

        let current_string = f.wait_timeout(one_sec).expect("GET");
        assert_eq!("default", current_string);
        rc_ref.tell(StringMsg::Put("MsgTest"));
        rc_ref.tell(StringMsg::Crash);

        thread::sleep(one_sec);

        // get new reference
        let rc_ref = RecovererComponent::get_current_ref().expect("ref");

        let (p, f) = promise::<&'static str>();

        rc_ref.tell(StringMsg::Get(p));

        let current_string = f.wait_timeout(one_sec).expect("GET");
        assert_eq!("MsgTest", current_string);

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
                ctx: ComponentContext::uninitialised(),
            }
        }
    }

    impl ComponentLifecycle for Stopper {
        fn on_start(&mut self) -> Handled {
            self.ctx().system().shutdown_async();
            Handled::Ok
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
                ctx: ComponentContext::uninitialised(),
                test_value: None,
            }
        }
    }

    impl ComponentLifecycle for ConfigComponent {
        fn on_start(&mut self) -> Handled {
            self.test_value = self.ctx().config()["a"].as_i64();
            self.ctx().system().shutdown_async();
            Handled::Ok
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

    #[test]
    fn test_system_spawn() -> () {
        let system = KompactConfig::default().build().expect("system");
        let handle = system.spawn(async move { "test".to_string() });
        let res = futures::executor::block_on(handle).expect("result");
        assert_eq!(res, "test");
    }
}

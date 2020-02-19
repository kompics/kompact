use super::*;

use crate::{
    messaging::{
        DispatchEnvelope,
        MsgEnvelope,
        PathResolvable,
        RegistrationEnvelope,
        RegistrationError,
    },
    supervision::{ComponentSupervisor, ListenEvent, SupervisionPort, SupervisorMsg},
};
use executors::*;
use hocon::{Hocon, HoconLoader};
use oncemutex::{OnceMutex, OnceMutexGuard};
use std::{
    clone::Clone,
    fmt::{Debug, Formatter, Result as FmtResult},
    path::PathBuf,
    rc::Rc,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
        Mutex,
        Once,
    },
};

static GLOBAL_RUNTIME_COUNT: AtomicUsize = AtomicUsize::new(0);

fn default_runtime_label() -> String {
    let runtime_count = GLOBAL_RUNTIME_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
    format!("kompact-runtime-{}", runtime_count)
}

static mut DEFAULT_ROOT_LOGGER: Option<KompactLogger> = None;
static DEFAULT_ROOT_LOGGER_INIT: Once = Once::new();

fn default_logger() -> &'static KompactLogger {
    unsafe {
        DEFAULT_ROOT_LOGGER_INIT.call_once(|| {
            let decorator = slog_term::TermDecorator::new().stdout().build();
            let drain = slog_term::FullFormat::new(decorator).build().fuse();
            let drain = slog_async::Async::new(drain).chan_size(1024).build().fuse();
            DEFAULT_ROOT_LOGGER = Some(slog::Logger::root_typed(
                Arc::new(drain),
                o!(
                "location" => slog::PushFnValue(|r: &slog::Record<'_>, ser: slog::PushFnValueSerializer<'_>| {
                    ser.emit(format_args!("{}:{}", r.file(), r.line()))
                })
                        ),
            ));
        });
        match DEFAULT_ROOT_LOGGER {
            Some(ref l) => l,
            None => unreachable!(),
        }
    }
}

type SchedulerBuilder = dyn Fn(usize) -> Box<dyn Scheduler>;

type SCBuilder = dyn Fn(&KompactSystem, Promise<()>, Promise<()>) -> Box<dyn SystemComponents>;

type TimerBuilder = dyn Fn() -> Box<dyn TimerComponent>;

/// A Kompact system error
#[derive(Debug, PartialEq, Clone)]
pub enum KompactError {
    /// A mutex in the system has been poisoned
    Poisoned,
    /// An error occurred loading the HOCON config
    ConfigError(hocon::Error),
}

impl From<hocon::Error> for KompactError {
    fn from(e: hocon::Error) -> Self {
        KompactError::ConfigError(e)
    }
}

#[derive(Debug, Clone)]
enum ConfigSource {
    File(PathBuf),
    Str(String),
}

/// A configuration builder for Kompact systems
///
/// # Example
///
/// Set a custom label, and run up to 50 events or messages per scheduling of a component
/// on a threadpool with 2 threads.
///
/// ```
/// use kompact::prelude::*;
///
/// let mut conf = KompactConfig::default();
/// conf.label("My special system")
///     .throughput(50)
///     .threads(2);
/// let system = conf.build().expect("system");
/// # system.shutdown().expect("shutdown");
/// ```
#[derive(Clone)]
pub struct KompactConfig {
    label: String,
    throughput: usize,
    msg_priority: f32,
    threads: usize,
    timer_builder: Rc<TimerBuilder>,
    scheduler_builder: Rc<SchedulerBuilder>,
    sc_builder: Rc<SCBuilder>,
    root_logger: Option<KompactLogger>,
    config_sources: Vec<ConfigSource>,
}

impl Debug for KompactConfig {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(
            f,
            "KompactConfig{{
            label={},
            throughput={},
            msg_priority={},
            threads={},
            timer_builder=<function>,
            scheduler_builder=<function>,
            sc_builder=<function>,
            root_logger={:?},
            config_sources={:?}
        }}",
            self.label,
            self.throughput,
            self.msg_priority,
            self.threads,
            self.root_logger,
            self.config_sources
        )
    }
}

impl KompactConfig {
    /// Create a minimal Kompact config
    ///
    /// The minimal config uses the default label, i.e. `"kompact-runtime-{}"` for some sequence number.
    /// It sets the event/message throughput to 2, split evenly between events and messages.
    /// It runs with a single thread on a [small pool](crossbeam_workstealing_pool::small_pool).
    /// It uses all default components, without networking, with the default timer and default logger.
    pub fn new() -> KompactConfig {
        KompactConfig {
            label: default_runtime_label(),
            throughput: 2,
            msg_priority: 0.5,
            threads: 1,
            timer_builder: Rc::new(|| DefaultTimer::new_timer_component()),
            scheduler_builder: Rc::new(|t| {
                ExecutorScheduler::from(crossbeam_workstealing_pool::small_pool(t))
            }),
            sc_builder: Rc::new(|sys, dead_prom, disp_prom| {
                Box::new(DefaultComponents::new(sys, dead_prom, disp_prom))
            }),
            root_logger: None,
            config_sources: Vec::new(),
        }
    }

    /// Set the name of the system
    ///
    /// The label is used as metadata for logging output.
    pub fn label<I>(&mut self, s: I) -> &mut Self
    where
        I: Into<String>,
    {
        self.label = s.into();
        self
    }

    /// Set the maximum number of events/messages to handle before rescheduling a component
    ///
    /// Larger values can increase throughput on highly loaded components,
    /// but at the cost of fairness between components.
    pub fn throughput(&mut self, n: usize) -> &mut Self {
        self.throughput = n;
        self
    }

    /// Set the ratio between handling messages and events.
    ///
    /// A component will handle up to throughput * r messages
    /// and throughput * (1-r) events before rescheduling.
    ///
    /// If there are less events or messages than alloted queued up
    /// the remaining allotment will be redistributed to the other type
    /// until all throughput is used up or no messages or events remain.
    pub fn msg_priority(&mut self, r: f32) -> &mut Self {
        self.msg_priority = r;
        self
    }

    /// The number of threads in the Kompact thread pool
    ///
    /// # Note
    ///
    /// You *must* ensure that the selected [scheduler](KompactConfig::scheduler) implementation
    /// can manage the given number of threads, if you customise this value!
    pub fn threads(&mut self, n: usize) -> &mut Self {
        self.threads = n;
        self
    }

    /// Set a particular scheduler implementation
    ///
    /// Takes a function `f` from the number of threads to a concrete scheduler implementation as argument.
    pub fn scheduler<E, F>(&mut self, f: F) -> &mut Self
    where
        E: Executor + Sync + 'static,
        F: Fn(usize) -> E + 'static,
    {
        let sb = move |t: usize| ExecutorScheduler::from(f(t));
        self.scheduler_builder = Rc::new(sb);
        self
    }

    /// Set a particular timer implementation
    pub fn timer<T, F>(&mut self, f: F) -> &mut Self
    where
        T: TimerComponent + 'static,
        F: Fn() -> Box<dyn TimerComponent> + 'static,
    {
        self.timer_builder = Rc::new(f);
        self
    }

    /// Set a particular set of system components
    ///
    /// In particular, this allows exchanging the default dispatcher for the
    /// [NetworkDispatcher](prelude::NetworkDispatcher), which enables the created Kompact system
    /// to perform network communication.
    ///
    /// # Example
    ///
    /// For using the network dispatcher, with the default deadletter box:
    /// ```
    /// use kompact::prelude::*;
    ///
    /// let mut cfg = KompactConfig::new();
    /// cfg.system_components(DeadletterBox::new, {
    ///     let net_config = NetworkConfig::new("127.0.0.1:0".parse().expect("Address should work"));
    ///     net_config.build()
    /// });
    /// let system = cfg.build().expect("KompactSystem");
    /// # system.shutdown().expect("shutdown");
    /// ```
    pub fn system_components<B, C, FB, FC>(
        &mut self,
        deadletter_fn: FB,
        dispatcher_fn: FC,
    ) -> &mut Self
    where
        B: ComponentDefinition + ActorRaw<Message = Never> + Sized + 'static,
        C: ComponentDefinition
            + ActorRaw<Message = DispatchEnvelope>
            + Sized
            + 'static
            + Dispatcher,
        FB: Fn(Promise<()>) -> B + 'static,
        FC: Fn(Promise<()>) -> C + 'static,
    {
        let sb = move |system: &KompactSystem, dead_prom: Promise<()>, disp_prom: Promise<()>| {
            let deadletter_box = system.create_unsupervised(|| deadletter_fn(dead_prom));
            let dispatcher = system.create_unsupervised(|| dispatcher_fn(disp_prom));

            let cc = CustomComponents {
                deadletter_box,
                dispatcher,
            };
            Box::new(cc) as Box<dyn SystemComponents>
        };
        self.sc_builder = Rc::new(sb);
        self
    }

    /// Set a particular set of system components
    ///
    /// This function works just like [system_components](KompactConfig::system_components),
    /// except that it assigns the dispatcher to its own thread using
    /// [create_dedicated_unsupervised](KompactSystem::create_dedicated_unsupervised).
    pub fn system_components_with_dedicated_dispatcher<B, C, FB, FC>(
        &mut self,
        deadletter_fn: FB,
        dispatcher_fn: FC,
    ) -> &mut Self
    where
        B: ComponentDefinition + ActorRaw<Message = Never> + Sized + 'static,
        C: ComponentDefinition
            + ActorRaw<Message = DispatchEnvelope>
            + Sized
            + 'static
            + Dispatcher,
        FB: Fn(Promise<()>) -> B + 'static,
        FC: Fn(Promise<()>) -> C + 'static,
    {
        let sb = move |system: &KompactSystem, dead_prom: Promise<()>, disp_prom: Promise<()>| {
            let deadletter_box = system.create_unsupervised(|| deadletter_fn(dead_prom));
            let dispatcher = system.create_dedicated_unsupervised(|| dispatcher_fn(disp_prom));

            let cc = CustomComponents {
                deadletter_box,
                dispatcher,
            };
            Box::new(cc) as Box<dyn SystemComponents>
        };
        self.sc_builder = Rc::new(sb);
        self
    }

    /// Set the logger implementation to use
    pub fn logger(&mut self, logger: KompactLogger) -> &mut Self {
        self.root_logger = Some(logger);
        self
    }

    /// Load a HOCON config from a file at `path`
    ///
    /// This method can be called multiple times, and the resulting configurations will be merged.
    /// See [HoconLoader](hocon::HoconLoader) for more information.
    ///
    /// The loaded config can be accessed via [`system.config()`](KompactSystem::config
    /// or from within a component via [`self.ctx().config()`](ComponentContext::config.
    pub fn load_config_file<P>(&mut self, path: P) -> &mut Self
    where
        P: Into<PathBuf>,
    {
        let p: PathBuf = path.into();
        self.config_sources.push(ConfigSource::File(p));
        self
    }

    /// Load a HOCON config from a string
    ///
    /// This method can be called multiple times, and the resulting configurations will be merged.
    /// See [HoconLoader](hocon::HoconLoader) for more information.
    ///
    /// The loaded config can be accessed via [`system.config()`](KompactSystem::config
    /// or from within a component via [`self.ctx().config()`](ComponentContext::config.
    pub fn load_config_str<S>(&mut self, config: S) -> &mut Self
    where
        S: Into<String>,
    {
        let s: String = config.into();
        self.config_sources.push(ConfigSource::Str(s));
        self
    }

    /// Finalise the config and use it create a [KompactSystem](KompactSystem)
    ///
    /// This function can fail, if the configuration sets up invalid schedulers
    /// or dispatchers, for example.
    ///
    /// # Example
    ///
    /// Build a system with default settings with:
    ///
    /// ```
    /// use kompact::prelude::*;
    ///
    /// let system = KompactConfig::default().build().expect("system");
    /// # system.shutdown().expect("shutdown");
    /// ```
    pub fn build(self) -> Result<KompactSystem, KompactError> {
        KompactSystem::try_new(self)
    }

    fn max_messages(&self) -> usize {
        let tpf = self.throughput as f32;
        let mmf = tpf * self.msg_priority;
        assert!(mmf >= 0.0, "msg_priority can not be negative!");
        let mm = mmf as usize;
        mm
    }
}

impl Default for KompactConfig {
    /// Create a default Kompact config
    ///
    /// The default config uses the default label, i.e. `"kompact-runtime-{}"` for some sequence number.
    /// It sets the event/message throughput to 50, split evenly between events and messages.
    /// It runs with one thread per cpu on an appropriately sized [`crossbeam_workstealing_pool`](crossbeam_workstealing_pool) implementation.
    /// It uses all default components, without networking, with the default timer and default logger.
    fn default() -> Self {
        let threads = num_cpus::get();
        let scheduler_builder: Rc<SchedulerBuilder> = if threads <= 32 {
            Rc::new(|t| ExecutorScheduler::from(crossbeam_workstealing_pool::small_pool(t)))
        } else if threads <= 64 {
            Rc::new(|t| ExecutorScheduler::from(crossbeam_workstealing_pool::large_pool(t)))
        } else {
            Rc::new(|t| ExecutorScheduler::from(crossbeam_workstealing_pool::dyn_pool(t)))
        };
        KompactConfig {
            label: default_runtime_label(),
            throughput: 50,
            msg_priority: 0.5,
            threads,
            timer_builder: Rc::new(|| DefaultTimer::new_timer_component()),
            scheduler_builder,
            sc_builder: Rc::new(|sys, dead_prom, disp_prom| {
                Box::new(DefaultComponents::new(sys, dead_prom, disp_prom))
            }),
            root_logger: None,
            config_sources: Vec::new(),
        }
    }
}

/// A Kompact system is a collection of components and services
///
/// An instance of `KompactSystem` is created from a [KompactConfig](KompactConfig)
/// via the [build](KompactConfig::build) function.
///
/// It is possible to run more than one Kompact system in a single process.
/// This allows different settings to be used for different component groups, for example.
/// It can also be used for testing communication in unit or integration tests.
///
/// # Note
///
/// For some schedulers it can happen that components may switch from one system's scheduler to the
/// other's when running multiple systems in the same process and communicating between them via channels
/// or actor references.
/// Generally, this shouldn't be an issue, but it can invalidate assumptions on thread assignments, so it's
/// important to be aware of. If this behaviour needs to be avoided at all costs, one can either use a scheduler
/// that doesn't use thread-local variables to determine the target queue
/// (e.g., [`crossbeam_channel_pool`](executors::crossbeam_channel_pool)),
/// or limit cross-system communication to network-only,
/// incurring the associated serialisation/deserialisations costs.
///
/// # Example
///
/// Build a system with default settings with:
///
/// ```
/// use kompact::prelude::*;
///
/// let system = KompactConfig::default().build().expect("system");
/// # system.shutdown().expect("shutdown");
/// ```
#[derive(Clone)]
pub struct KompactSystem {
    inner: Arc<KompactRuntime>,
    config: Arc<Hocon>,
    scheduler: Box<dyn Scheduler>,
}

impl KompactSystem {
    /// Use the [build](KompactConfig::build) method instead.
    pub(crate) fn try_new(conf: KompactConfig) -> Result<Self, KompactError> {
        let scheduler = (*conf.scheduler_builder)(conf.threads);
        let sc_builder = conf.sc_builder.clone();
        let config_loader_initial: Result<HoconLoader, hocon::Error> =
            Result::Ok(HoconLoader::new());
        let config = conf
            .config_sources
            .iter()
            .fold(config_loader_initial, |config_loader, source| {
                config_loader.and_then(|cl| match source {
                    ConfigSource::File(path) => cl.load_file(path),
                    ConfigSource::Str(s) => cl.load_str(s),
                })
            })?
            .hocon()?;
        let runtime = Arc::new(KompactRuntime::new(conf));
        let sys = KompactSystem {
            inner: runtime,
            config: Arc::new(config),
            scheduler,
        };
        let (dead_prom, dead_f) = utils::promise();
        let (disp_prom, disp_f) = utils::promise();
        let system_components = (*sc_builder)(&sys, dead_prom, disp_prom);
        let supervisor = sys.create_unsupervised(ComponentSupervisor::new);
        let ic = InternalComponents::new(supervisor, system_components);
        sys.inner.set_internal_components(ic);
        sys.inner.start_internal_components(&sys);
        let timeout = std::time::Duration::from_millis(50);
        let mut wait_for: Option<Future<()>> = Some(dead_f);
        while wait_for.is_some() {
            if sys.inner.is_poisoned() {
                return Err(KompactError::Poisoned);
            }
            match wait_for.take().unwrap().wait_timeout(timeout) {
                Ok(_) => (),
                Err(w) => wait_for = Some(w),
            }
        }
        let mut wait_for: Option<Future<()>> = Some(disp_f);
        while wait_for.is_some() {
            if sys.inner.is_poisoned() {
                return Err(KompactError::Poisoned);
            }
            match wait_for.take().unwrap().wait_timeout(timeout) {
                Ok(_) => (),
                Err(w) => wait_for = Some(w),
            }
        }
        Ok(sys)
    }

    pub(crate) fn schedule(&self, c: Arc<dyn CoreContainer>) -> () {
        self.scheduler.schedule(c);
    }

    /// Get a reference to the system-wide Kompact logger
    ///
    /// # Example
    ///
    /// ```
    /// # use kompact::prelude::*;
    /// # let system = KompactConfig::default().build().expect("system");
    /// info!(system.logger(), "Hello World from the system logger!");
    /// # system.shutdown().expect("shutdown");
    /// ```
    pub fn logger(&self) -> &KompactLogger {
        &self.inner.logger
    }

    /// Get a reference to the system configuration
    ///
    /// Use [load_config_str](KompactConfig::load_config_str) or
    /// or [load_config_file](KompactConfig::load_config_file)
    /// to load values into the config object.
    ///
    /// # Example
    ///
    /// ```
    /// use kompact::prelude::*;
    /// let default_values = r#"{ a = 7 }"#;
    /// let mut conf = KompactConfig::default();
    /// conf.load_config_str(default_values);
    /// let system = conf.build().expect("system");
    /// assert_eq!(Some(7i64), system.config()["a"].as_i64());
    /// ```
    pub fn config(&self) -> &Hocon {
        self.config.as_ref()
    }

    /// Get a owned reference to the system configuration
    pub fn config_owned(&self) -> Arc<Hocon> {
        self.config.clone()
    }

    pub(crate) fn poison(&self) {
        self.inner.poison();
        self.scheduler.poison();
    }

    /// Create a new component
    ///
    /// Uses `f` to create an instance of a [ComponentDefinition](ComponentDefinition),
    /// which is the initialised to form a [Component](Component).
    /// Since components are shared between threads, the created component
    /// is wrapped into an [Arc](std::sync::Arc).
    ///
    /// Newly created components are not started automatically.
    /// Use [start](KompactSystem::start) or [start_notify](KompactSystem::start_notify)
    /// to start a newly created component, once it is connected properly.
    ///
    /// If you need address this component via the network, see the [register](KompactSystem::register) function.
    ///
    /// # Example
    ///
    /// ```
    /// # use kompact::prelude::*;
    /// # use kompact::doctest_helpers::*;
    /// # let system = KompactConfig::default().build().expect("system");
    /// let c = system.create(TestComponent1::new);
    /// # system.shutdown().expect("shutdown");
    /// ```
    pub fn create<C, F>(&self, f: F) -> Arc<Component<C>>
    where
        F: FnOnce() -> C,
        C: ComponentDefinition + 'static,
    {
        self.inner.assert_active();
        let c = Arc::new(Component::new(self.clone(), f(), self.supervision_port()));
        unsafe {
            let mut cd = c.definition().lock().unwrap();
            let cc: Arc<dyn CoreContainer> = c.clone() as Arc<dyn CoreContainer>;
            cd.setup(c.clone());
            c.core().set_component(cc);
        }
        return c;
    }

    /// Create a new system component
    ///
    /// You *must* use this instead of [create](KompactSystem::create) to create
    /// a new system component.
    /// During system initialisation the supervisor is not available, yet,
    /// so normal [create](KompactSystem::create) calls will panic!
    pub fn create_unsupervised<C, F>(&self, f: F) -> Arc<Component<C>>
    where
        F: FnOnce() -> C,
        C: ComponentDefinition + 'static,
    {
        let c = Arc::new(Component::without_supervisor(self.clone(), f()));
        unsafe {
            let mut cd = c.definition().lock().unwrap();
            cd.setup(c.clone());
            let cc: Arc<dyn CoreContainer> = c.clone() as Arc<dyn CoreContainer>;
            c.core().set_component(cc);
        }
        return c;
    }

    /// Create a new component, which runs on its own dedicated thread.
    ///
    /// Uses `f` to create an instance of a [ComponentDefinition](ComponentDefinition),
    /// which is the initialised to form a [Component](Component).
    /// Since components are shared between threads, the created component
    /// is wrapped into an [Arc](std::sync::Arc).
    ///
    /// A dedicated thread is assigned to this component, which sleeps when the component has no work.
    ///
    /// Newly created components are not started automatically.
    /// Use [start](KompactSystem::start) or [start_notify](KompactSystem::start_notify)
    /// to start a newly created component, once it is connected properly.
    ///
    /// If you need address this component via the network, see the [register](KompactSystem::register) function.
    pub fn create_dedicated<C, F>(&self, f: F) -> Arc<Component<C>>
    where
        F: FnOnce() -> C,
        C: ComponentDefinition + 'static,
    {
        self.inner.assert_active();
        let (scheduler, promise) =
            dedicated_scheduler::DedicatedThreadScheduler::new().expect("Scheduler");
        let c = Arc::new(Component::with_dedicated_scheduler(
            self.clone(),
            f(),
            self.supervision_port(),
            scheduler,
        ));
        unsafe {
            let mut cd = c.definition().lock().unwrap();
            let cc: Arc<dyn CoreContainer> = c.clone() as Arc<dyn CoreContainer>;
            cd.setup(c.clone());
            c.core().set_component(cc);
        }
        promise.fulfill(c.clone()).expect("Should accept component");
        return c;
    }

    /// Create a new component, which runs on its own dedicated thread and is pinned to certain CPU core.
    ///
    /// This functionality is only available with feature `thread_pinning`.
    ///
    /// Uses `f` to create an instance of a [ComponentDefinition](ComponentDefinition),
    /// which is the initialised to form a [Component](Component).
    /// Since components are shared between threads, the created component
    /// is wrapped into an [Arc](std::sync::Arc).
    ///
    /// A dedicated thread is assigned to this component, which sleeps when the component has no work.
    /// The thread is also pinned to the given `core_id`, allowing static, manual, NUMA aware component assignments.
    ///
    /// Newly created components are not started automatically.
    /// Use [start](KompactSystem::start) or [start_notify](KompactSystem::start_notify)
    /// to start a newly created component, once it is connected properly.
    ///
    /// If you need address this component via the network, see the [register](KompactSystem::register) function.
    #[cfg(feature = "thread_pinning")]
    pub fn create_dedicated_pinned<C, F>(&self, f: F, core_id: CoreId) -> Arc<Component<C>>
    where
        F: FnOnce() -> C,
        C: ComponentDefinition + 'static,
    {
        self.inner.assert_active();
        let (scheduler, promise) =
            dedicated_scheduler::DedicatedThreadScheduler::pinned(core_id).expect("Scheduler");
        let c = Arc::new(Component::with_dedicated_scheduler(
            self.clone(),
            f(),
            self.supervision_port(),
            scheduler,
        ));
        unsafe {
            let mut cd = c.definition().lock().unwrap();
            let cc: Arc<dyn CoreContainer> = c.clone() as Arc<dyn CoreContainer>;
            cd.setup(c.clone());
            c.core().set_component(cc);
        }
        promise.fulfill(c.clone()).expect("Should accept component");
        return c;
    }

    /// Create a new system component, which runs on its own dedicated thread.
    ///
    /// A dedicated thread is assigned to this component, which sleeps when the component has no work.
    ///
    /// You *must* use this instead of [create_dedicated](KompactSystem::create_dedicated) to create
    /// a new system component on its own thread.
    /// During system initialisation the supervisor is not available, yet,
    /// so normal [create](KompactSystem::create) calls will panic!
    pub fn create_dedicated_unsupervised<C, F>(&self, f: F) -> Arc<Component<C>>
    where
        F: FnOnce() -> C,
        C: ComponentDefinition + 'static,
    {
        let (scheduler, promise) =
            dedicated_scheduler::DedicatedThreadScheduler::new().expect("Scheduler");
        let c = Arc::new(Component::without_supervisor_with_dedicated_scheduler(
            self.clone(),
            f(),
            scheduler,
        ));
        unsafe {
            let mut cd = c.definition().lock().unwrap();
            cd.setup(c.clone());
            let cc: Arc<dyn CoreContainer> = c.clone() as Arc<dyn CoreContainer>;
            c.core().set_component(cc);
        }
        promise.fulfill(c.clone()).expect("Should accept component");
        return c;
    }

    /// Create a new system component, which runs on its own dedicated thread.
    ///
    /// A dedicated thread is assigned to this component, which sleeps when the component has no work.
    /// The thread is also pinned to the given `core_id`, allowing static, manual, NUMA aware component assignments.
    ///
    /// You *must* use this instead of [create_dedicated_pinned](KompactSystem::create_dedicated_pinned) to create
    /// a new system component on its own thread.
    /// During system initialisation the supervisor is not available, yet,
    /// so normal [create](KompactSystem::create) calls will panic!
    #[cfg(feature = "thread_pinning")]
    pub fn create_dedicated_pinned_unsupervised<C, F>(
        &self,
        f: F,
        core_id: CoreId,
    ) -> Arc<Component<C>>
    where
        F: FnOnce() -> C,
        C: ComponentDefinition + 'static,
    {
        let (scheduler, promise) =
            dedicated_scheduler::DedicatedThreadScheduler::pinned(core_id).expect("Scheduler");
        let c = Arc::new(Component::without_supervisor_with_dedicated_scheduler(
            self.clone(),
            f(),
            scheduler,
        ));
        unsafe {
            let mut cd = c.definition().lock().unwrap();
            cd.setup(c.clone());
            let cc: Arc<dyn CoreContainer> = c.clone() as Arc<dyn CoreContainer>;
            c.core().set_component(cc);
        }
        promise.fulfill(c.clone()).expect("Should accept component");
        return c;
    }

    /// Attempts to register `c` with the dispatcher using its unique id
    ///
    /// The returned future will contain the unique id [ActorPath](ActorPath)
    /// for the given component, once it is completed by the dispatcher.
    ///
    /// Once the future completes, the component can be addressed via the network,
    /// even if it has not been started, yet (in which case messages will simply be queued up).
    ///
    /// # Example
    ///
    /// ```
    /// use kompact::prelude::*;
    /// # use kompact::doctest_helpers::*;
    /// use std::time::Duration;
    /// let mut cfg = KompactConfig::new();
    /// cfg.system_components(DeadletterBox::new, {
    ///     let net_config = NetworkConfig::new("127.0.0.1:0".parse().expect("Address should work"));
    ///     net_config.build()
    /// });
    /// let system = cfg.build().expect("KompactSystem");
    /// let c = system.create(TestComponent1::new);
    /// system.register(&c).wait_expect(Duration::from_millis(1000), "Failed to register TestComponent1");
    /// # system.shutdown().expect("shutdown");
    /// ```
    pub fn register<C>(&self, c: &Arc<Component<C>>) -> Future<Result<ActorPath, RegistrationError>>
    where
        C: ComponentDefinition + 'static,
    {
        self.inner.assert_active();
        let id = c.core().id().clone();
        let id_path = PathResolvable::ActorId(id);
        self.inner.register_by_path(c, id_path)
    }

    /// Creates a new component and registers it with the dispatcher
    ///
    /// This function is simply a convenience shortcut for
    /// [create](KompactSystem::create) followed by [register](KompactSystem::register),
    /// as this combination is very common in networked Kompact systems.
    ///
    /// # Example
    ///
    /// ```
    /// use kompact::prelude::*;
    /// # use kompact::doctest_helpers::*;
    /// use std::time::Duration;
    /// let mut cfg = KompactConfig::new();
    /// cfg.system_components(DeadletterBox::new, {
    ///     let net_config = NetworkConfig::new("127.0.0.1:0".parse().expect("Address should work"));
    ///     net_config.build()
    /// });
    /// let system = cfg.build().expect("KompactSystem");
    /// let (c, registration_future) = system.create_and_register(TestComponent1::new);
    /// registration_future.wait_expect(Duration::from_millis(1000), "Failed to register TestComponent1");
    /// # system.shutdown().expect("shutdown");
    /// ```
    pub fn create_and_register<C, F>(
        &self,
        f: F,
    ) -> (
        Arc<Component<C>>,
        Future<Result<ActorPath, RegistrationError>>,
    )
    where
        F: FnOnce() -> C,
        C: ComponentDefinition + 'static,
    {
        // it will already check this twice...don't check it a third time
        // self.inner.assert_active();
        let c = self.create(f);
        let r = self.register(&c);
        (c, r)
    }

    // NOTE this was a terribly inconsistent API which hid a lot of possible failures away.
    // If we feel we need a shortcut for create, register, start in the future, we should design
    // a clearer and more reliable way of doing so.
    // pub fn create_and_start<C, F>(&self, f: F) -> Arc<Component<C>>
    // where
    //     F: FnOnce() -> C,
    //     C: ComponentDefinition + 'static,
    // {
    //     self.inner.assert_active();
    //     let c = self.create(f);
    //     let path = PathResolvable::ActorId(c.core().id().clone());
    //     self.inner.register_by_path(&c, path);
    //     self.start(&c);
    //     c
    // }

    /// Attempts to register the provided component with a human-readable alias.
    ///
    /// The returned future will contain the named [ActorPath](ActorPath)
    /// for the given alias, once it is completed by the dispatcher.
    ///
    /// # Note
    ///
    /// While aliases are easier to read, lookup by unique ids is significantly more efficient.
    /// However, named aliases allow services to be taken over by another component when the original registrant failed,
    /// something that is not possible with unique paths. Thus, this kind of addressing lends itself to lookup-service
    /// style components, for example.
    ///
    /// # Example
    ///
    /// ```
    /// use kompact::prelude::*;
    /// # use kompact::doctest_helpers::*;
    /// use std::time::Duration;
    /// let mut cfg = KompactConfig::new();
    /// cfg.system_components(DeadletterBox::new, {
    ///     let net_config = NetworkConfig::new("127.0.0.1:0".parse().expect("Address should work"));
    ///     net_config.build()
    /// });
    /// let system = cfg.build().expect("KompactSystem");
    /// let (c, unique_registration_future) = system.create_and_register(TestComponent1::new);
    /// unique_registration_future.wait_expect(Duration::from_millis(1000), "Failed to register TestComponent1");
    /// let alias_registration_future = system.register_by_alias(&c, "test");
    /// alias_registration_future.wait_expect(Duration::from_millis(1000), "Failed to register TestComponent1 by alias");
    /// # system.shutdown().expect("shutdown");
    /// ```
    pub fn register_by_alias<C, A>(
        &self,
        c: &Arc<Component<C>>,
        alias: A,
    ) -> Future<Result<ActorPath, RegistrationError>>
    where
        C: ComponentDefinition + 'static,
        A: Into<String>,
    {
        self.inner.assert_active();
        self.inner.register_by_alias(c, alias.into())
    }

    /// Start a component
    ///
    /// A component only handles events/messages once it is started.
    /// In particular, a component that isn't started shouldn't be scheduled and thus
    /// access to its definition should always succeed,
    /// for example via [on_definition](Component::on_definition).
    ///
    /// # Example
    ///
    /// ```
    /// # use kompact::prelude::*;
    /// # use kompact::doctest_helpers::*;
    /// # let system = KompactConfig::default().build().expect("system");
    /// let c = system.create(TestComponent1::new);
    /// system.start(&c);
    /// # system.shutdown().expect("shutdown");
    /// ```
    pub fn start<C>(&self, c: &Arc<Component<C>>) -> ()
    where
        C: ComponentDefinition + 'static,
    {
        self.inner.assert_not_poisoned();
        c.enqueue_control(ControlEvent::Start);
    }

    /// Start a component and complete a future once it has started
    ///
    /// When the returned future completes, the component is guaranteed to have started.
    /// However, it is not guaranteed to be in an active state,
    /// as it could already have been stopped or could have failed since.
    ///
    /// A component only handles events/messages once it is started.
    /// In particular, a component that isn't started shouldn't be scheduled and thus
    /// access to its definition should always succeed,
    /// for example via [on_definition](Component::on_definition).
    ///
    /// # Example
    ///
    /// ```
    /// # use kompact::prelude::*;
    /// # use kompact::doctest_helpers::*;
    /// use std::time::Duration;
    /// # let system = KompactConfig::default().build().expect("system");
    /// let c = system.create(TestComponent1::new);
    /// system.start_notify(&c)
    ///       .wait_timeout(Duration::from_millis(1000))
    ///       .expect("TestComponent1 never started!");
    /// # system.shutdown().expect("shutdown");
    /// ```
    pub fn start_notify<C>(&self, c: &Arc<Component<C>>) -> Future<()>
    where
        C: ComponentDefinition + 'static,
    {
        self.inner.assert_active();
        let (p, f) = utils::promise();
        let amp = Arc::new(Mutex::new(p));
        self.supervision_port().enqueue(SupervisorMsg::Listen(
            amp,
            ListenEvent::Started(c.id().clone()),
        ));
        c.enqueue_control(ControlEvent::Start);
        f
    }

    /// Stop a component
    ///
    /// A component does not handle any events/messages while it is stopped,
    /// but it does not get deallocated either. It can be started again later with
    /// [start](KompactSystem::start) or [start_notify](KompactSystem::start_notify).
    ///
    /// A component that is stopped shouldn't be scheduled and thus
    /// access to its definition should always succeed,
    /// for example via [on_definition](Component::on_definition).
    ///
    /// # Example
    ///
    /// ```
    /// # use kompact::prelude::*;
    /// # use kompact::doctest_helpers::*;
    /// use std::time::Duration;
    /// # let system = KompactConfig::default().build().expect("system");
    /// let c = system.create(TestComponent1::new);
    /// system.start_notify(&c)
    ///       .wait_timeout(Duration::from_millis(1000))
    ///       .expect("TestComponent1 never started!");
    /// system.stop(&c);
    /// # system.shutdown().expect("shutdown");
    /// ```
    pub fn stop<C>(&self, c: &Arc<Component<C>>) -> ()
    where
        C: ComponentDefinition + 'static,
    {
        self.inner.assert_active();
        c.enqueue_control(ControlEvent::Stop);
    }

    /// Stop a component and complete a future once it has stopped
    ///
    /// When the returned future completes, the component is guaranteed to have stopped.
    /// However, it is not guaranteed to be in a passive state,
    /// as it could already have been started again since.
    ///
    /// A component does not handle any events/messages while it is stopped,
    /// but it does not get deallocated either. It can be started again later with
    /// [start](KompactSystem::start) or [start_notify](KompactSystem::start_notify).
    ///
    /// A component that is stopped shouldn't be scheduled and thus
    /// access to its definition should always succeed,
    /// for example via [on_definition](Component::on_definition).
    ///
    /// # Example
    ///
    /// ```
    /// # use kompact::prelude::*;
    /// # use kompact::doctest_helpers::*;
    /// use std::time::Duration;
    /// # let system = KompactConfig::default().build().expect("system");
    /// let c = system.create(TestComponent1::new);
    /// system.start_notify(&c)
    ///       .wait_timeout(Duration::from_millis(1000))
    ///       .expect("TestComponent1 never started!");
    /// system.stop_notify(&c)
    ///       .wait_timeout(Duration::from_millis(1000))
    ///       .expect("TestComponent1 never stopped!");
    /// system.start_notify(&c)
    ///       .wait_timeout(Duration::from_millis(1000))
    ///       .expect("TestComponent1 never re-started!");
    /// # system.shutdown().expect("shutdown");
    /// ```
    pub fn stop_notify<C>(&self, c: &Arc<Component<C>>) -> Future<()>
    where
        C: ComponentDefinition + 'static,
    {
        self.inner.assert_active();
        let (p, f) = utils::promise();
        let amp = Arc::new(Mutex::new(p));
        self.supervision_port().enqueue(SupervisorMsg::Listen(
            amp,
            ListenEvent::Stopped(c.id().clone()),
        ));
        c.enqueue_control(ControlEvent::Stop);
        f
    }

    /// Stop and deallocate a component
    ///
    /// The supervisor will attempt to deallocate `c` once it is stopped.
    /// However, if there are still outstanding references somewhere else in the system
    /// this will fail, of course. In that case the supervisor leaves a debug message
    /// in the logging output, so that this circumstance can be discovered if necessary.
    ///
    /// # Example
    ///
    /// ```
    /// # use kompact::prelude::*;
    /// # use kompact::doctest_helpers::*;
    /// use std::time::Duration;
    /// # let system = KompactConfig::default().build().expect("system");
    /// let c = system.create(TestComponent1::new);
    /// system.start_notify(&c)
    ///       .wait_timeout(Duration::from_millis(1000))
    ///       .expect("TestComponent1 never started!");
    /// system.kill(c);
    /// # system.shutdown().expect("shutdown");
    /// ```
    pub fn kill<C>(&self, c: Arc<Component<C>>) -> ()
    where
        C: ComponentDefinition + 'static,
    {
        self.inner.assert_active();
        c.enqueue_control(ControlEvent::Kill);
    }

    /// Stop and deallocate a component, and complete a future once it has stopped
    ///
    /// The supervisor will attempt to deallocate `c` once it is stopped.
    /// However, if there are still outstanding references somewhere else in the system
    /// this will fail, of course. In that case the supervisor leaves a debug message
    /// in the logging output, so that this circumstance can be discovered if necessary.
    ///
    /// # Note
    ///
    /// The completion of the future indicates that the component has been stopped,
    /// *not* that it has been deallocated.
    ///
    /// If, for some reason, you really need to know when it has been deallocated,
    /// you need to hold on to a copy of the component, use [try_unwrap](std::sync::Arc::try_unwrap)
    /// and then call `drop` once you are successful.
    ///
    /// # Example
    ///
    /// ```
    /// # use kompact::prelude::*;
    /// # use kompact::doctest_helpers::*;
    /// use std::time::Duration;
    /// # let system = KompactConfig::default().build().expect("system");
    /// let c = system.create(TestComponent1::new);
    /// system.start_notify(&c)
    ///       .wait_timeout(Duration::from_millis(1000))
    ///       .expect("TestComponent1 never started!");
    /// system.kill_notify(c)
    ///       .wait_timeout(Duration::from_millis(1000))
    ///       .expect("TestComponent1 never stopped!");
    /// # system.shutdown().expect("shutdown");
    /// ```
    pub fn kill_notify<C>(&self, c: Arc<Component<C>>) -> Future<()>
    where
        C: ComponentDefinition + 'static,
    {
        self.inner.assert_active();
        let (p, f) = utils::promise();
        let amp = Arc::new(Mutex::new(p));
        self.supervision_port().enqueue(SupervisorMsg::Listen(
            amp,
            ListenEvent::Destroyed(c.id().clone()),
        ));
        c.enqueue_control(ControlEvent::Kill);
        f
    }

    /// Trigger an indication `event` on a shared `port`
    ///
    /// This can be used to send events to component without connecting a channel.
    /// You only nneed to acquire port reference via [share](prelude::RequiredPort::share).
    pub fn trigger_i<P>(&self, event: P::Indication, port: &RequiredRef<P>)
    where
        P: Port + 'static,
    {
        self.inner.assert_active();
        port.enqueue(event);
    }

    /// Trigger a request `event` on a shared `port`
    ///
    /// This can be used to send events to component without connecting a channel.
    /// You only nneed to acquire port reference via [share](prelude::ProvidedPort::share).
    pub fn trigger_r<P>(&self, msg: P::Request, port: &ProvidedRef<P>)
    where
        P: Port + 'static,
    {
        self.inner.assert_active();
        port.enqueue(msg);
    }

    /// Return the configured thoughput value
    ///
    /// See also [throughput](KompactConfig::throughput).
    pub fn throughput(&self) -> usize {
        self.inner.throughput
    }

    /// Return the configured maximum number of messages per scheduling
    ///
    /// This value is based on [throughput](KompactConfig::throughput)
    /// and [msg_priority](KompactConfig::msg_priority).
    pub fn max_messages(&self) -> usize {
        self.inner.max_messages
    }

    /// Wait for the Kompact system to be terminated
    ///
    /// Suspends this thread until the system is terminated
    /// from some other thread, such as its own threadpool,
    /// for example.
    ///
    /// # Note
    ///
    /// Don't use this method for any measurements,
    /// as its implementation currently does not include any
    /// notification logic. It simply checks its internal state
    /// every second or so, so there might be quite some delay
    /// until a shutdown is detected.
    pub fn await_termination(self) {
        loop {
            if lifecycle::is_destroyed(self.inner.state())
                || lifecycle::is_faulty(self.inner.state())
            {
                return;
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    }

    /// Shutdown the Kompact system
    ///
    /// Stops all components and then stops the scheduler.
    ///
    /// This function may fail to stop in time (or at all),
    /// if components hang on to scheduler threads indefinitely.
    ///
    /// # Example
    ///
    /// ```
    /// use kompact::prelude::*;
    ///
    /// let system = KompactConfig::default().build().expect("system");
    /// system.shutdown().expect("shutdown");
    /// ```
    pub fn shutdown(self) -> Result<(), String> {
        self.inner.assert_active();
        self.inner.shutdown(&self)?;
        self.scheduler.shutdown()?;
        Ok(())
    }

    /// Shutdown the Kompact system from within a component
    ///
    /// Stops all components and then stops the scheduler.
    ///
    /// This function may fail to stop in time (or at all),
    /// if components hang on to scheduler threads indefinitely.
    ///
    /// # Example
    ///
    /// ```
    /// use kompact::prelude::*;
    ///
    /// #[derive(ComponentDefinition, Actor)]
    /// struct Stopper {
    ///    ctx: ComponentContext<Self>,
    /// }
    /// impl Stopper {
    ///     fn new() -> Stopper {
    ///         Stopper {
    ///             ctx: ComponentContext::new(),
    ///         }
    ///     }    
    /// }
    /// impl Provide<ControlPort> for Stopper {
    ///    fn handle(&mut self, event: ControlEvent) -> () {
    ///         match event {
    ///            ControlEvent::Start => {
    ///                self.ctx().system().shutdown_async();
    ///            }
    ///            _ => (), // ignore
    ///        }
    ///     }
    /// }
    /// let system = KompactConfig::default().build().expect("system");
    /// let c = system.create(Stopper::new);
    /// system.start(&c);
    /// system.await_termination();
    /// ```
    pub fn shutdown_async(&self) -> () {
        let sys = self.clone();
        std::thread::spawn(move || {
            sys.shutdown().expect("shutdown");
        });
    }

    /// Return the system path of this Kompact system
    ///
    /// The system path forms a prefix for every [ActorPath](prelude::ActorPath).
    pub fn system_path(&self) -> SystemPath {
        self.inner.assert_active();
        self.inner.system_path()
    }

    /// Generate an unique path for the given component
    ///
    /// Produces a unique id [ActorPath](prelude::ActorPath) for `component`
    /// using the system path of this system.
    ///
    /// The returned `ActorPath` is only useful if the component was first [registered](KompactSystem::register),
    /// otherwise all messages sent to it will land in the system's deadletter box.
    ///
    /// # Note
    ///
    /// If you pass in a component from a different system, you will get
    /// a perfectly valid `ActorPath` instance, but which can not receive any messages,
    /// unless you also registered the component with this system's dispatcher.
    /// Suffice to say, crossing system boundaries in this manner is not recommended.
    pub fn actor_path_for<C>(&self, component: &Arc<Component<C>>) -> ActorPath
    where
        C: ComponentDefinition + 'static,
    {
        let id = *component.id();
        ActorPath::Unique(UniquePath::with_system(self.system_path(), id))
    }

    pub(crate) fn supervision_port(&self) -> ProvidedRef<SupervisionPort> {
        self.inner.supervision_port()
    }
}

impl ActorRefFactory for KompactSystem {
    type Message = Never;

    /// Returns a reference to the deadletter box
    fn actor_ref(&self) -> ActorRef<Never> {
        self.inner.assert_active();
        self.inner.deadletter_ref()
    }
}

impl Dispatching for KompactSystem {
    fn dispatcher_ref(&self) -> DispatcherRef {
        self.inner.assert_active();
        self.inner.dispatcher_ref()
    }
}

impl ActorSource for KompactSystem {
    fn path_resolvable(&self) -> PathResolvable {
        PathResolvable::System
    }
}

impl TimerRefFactory for KompactSystem {
    fn timer_ref(&self) -> timer::TimerRef {
        self.inner.assert_not_poisoned();
        self.inner.timer_ref()
    }
}

/// A limited version of a [KompactSystem](KompactSystem)
///
/// This is meant for use from within components, where all the blocking APIs
/// are unacceptable anyway.
pub trait SystemHandle: Dispatching {
    // TODO convert all the Future APIs either into message-based (e.g., WithSender)
    // or convert to Async/Await, once we have support for that.

    /// Create a new component
    ///
    /// Uses `f` to create an instance of a [ComponentDefinition](ComponentDefinition),
    /// which is the initialised to form a [Component](Component).
    /// Since components are shared between threads, the created component
    /// is wrapped into an [Arc](std::sync::Arc).
    ///
    /// Newly created components are not started automatically.
    /// Use [start](KompactSystem::start) or [start_notify](KompactSystem::start_notify)
    /// to start a newly created component, once it is connected properly.
    ///
    /// If you need address this component via the network, see the [register](KompactSystem::register) function.
    ///
    /// # Example
    ///
    /// ```
    /// # use kompact::prelude::*;
    /// # use kompact::doctest_helpers::*;
    /// # let system = KompactConfig::default().build().expect("system");
    /// let c = system.create(TestComponent1::new);
    /// # system.shutdown().expect("shutdown");
    /// ```
    fn create<C, F>(&self, f: F) -> Arc<Component<C>>
    where
        F: FnOnce() -> C,
        C: ComponentDefinition + 'static;

    /// Start a component
    ///
    /// A component only handles events/messages once it is started.
    /// In particular, a component that isn't started shouldn't be scheduled and thus
    /// access to its definition should always succeed,
    /// for example via [on_definition](Component::on_definition).
    ///
    /// # Example
    ///
    /// ```
    /// # use kompact::prelude::*;
    /// # use kompact::doctest_helpers::*;
    /// # let system = KompactConfig::default().build().expect("system");
    /// let c = system.create(TestComponent1::new);
    /// system.start(&c);
    /// # system.shutdown().expect("shutdown");
    /// ```
    fn start<C>(&self, c: &Arc<Component<C>>) -> ()
    where
        C: ComponentDefinition + 'static;

    /// Stop a component
    ///
    /// A component does not handle any events/messages while it is stopped,
    /// but it does not get deallocated either. It can be started again later with
    /// [start](KompactSystem::start) or [start_notify](KompactSystem::start_notify).
    ///
    /// A component that is stopped shouldn't be scheduled and thus
    /// access to its definition should always succeed,
    /// for example via [on_definition](Component::on_definition).
    ///
    /// # Example
    ///
    /// ```
    /// # use kompact::prelude::*;
    /// # use kompact::doctest_helpers::*;
    /// use std::time::Duration;
    /// # let system = KompactConfig::default().build().expect("system");
    /// let c = system.create(TestComponent1::new);
    /// system.start_notify(&c)
    ///       .wait_timeout(Duration::from_millis(1000))
    ///       .expect("TestComponent1 never started!");
    /// system.stop(&c);
    /// # system.shutdown().expect("shutdown");
    /// ```
    fn stop<C>(&self, c: &Arc<Component<C>>) -> ()
    where
        C: ComponentDefinition + 'static;

    /// Stop and deallocate a component
    ///
    /// The supervisor will attempt to deallocate `c` once it is stopped.
    /// However, if there are still outstanding references somewhere else in the system
    /// this will fail, of course. In that case the supervisor leaves a debug message
    /// in the logging output, so that this circumstance can be discovered if necessary.
    ///
    /// # Example
    ///
    /// ```
    /// # use kompact::prelude::*;
    /// # use kompact::doctest_helpers::*;
    /// use std::time::Duration;
    /// # let system = KompactConfig::default().build().expect("system");
    /// let c = system.create(TestComponent1::new);
    /// system.start_notify(&c)
    ///       .wait_timeout(Duration::from_millis(1000))
    ///       .expect("TestComponent1 never started!");
    /// system.kill(c);
    /// # system.shutdown().expect("shutdown");
    /// ```
    fn kill<C>(&self, c: Arc<Component<C>>) -> ()
    where
        C: ComponentDefinition + 'static;

    /// Return the configured thoughput value
    ///
    /// See also [throughput](KompactConfig::throughput).
    fn throughput(&self) -> usize;

    /// Return the configured maximum number of messages per scheduling
    ///
    /// This value is based on [throughput](KompactConfig::throughput)
    /// and [msg_priority](KompactConfig::msg_priority).
    fn max_messages(&self) -> usize;

    /// Shutdown the Kompact system from within a component
    ///
    /// Stops all components and then stops the scheduler.
    ///
    /// This function may fail to stop in time (or at all),
    /// if components hang on to scheduler threads indefinitely.
    ///
    /// # Example
    ///
    /// ```
    /// use kompact::prelude::*;
    ///
    /// #[derive(ComponentDefinition, Actor)]
    /// struct Stopper {
    ///    ctx: ComponentContext<Self>,
    /// }
    /// impl Stopper {
    ///     fn new() -> Stopper {
    ///         Stopper {
    ///             ctx: ComponentContext::new(),
    ///         }
    ///     }    
    /// }
    /// impl Provide<ControlPort> for Stopper {
    ///    fn handle(&mut self, event: ControlEvent) -> () {
    ///         match event {
    ///            ControlEvent::Start => {
    ///                self.ctx().system().shutdown_async();
    ///            }
    ///            _ => (), // ignore
    ///        }
    ///     }
    /// }
    /// let system = KompactConfig::default().build().expect("system");
    /// let c = system.create(Stopper::new);
    /// system.start(&c);
    /// system.await_termination();
    /// ```
    fn shutdown_async(&self) -> ();

    /// Return the system path of this Kompact system
    ///
    /// The system path forms a prefix for every [ActorPath](prelude::ActorPath).
    fn system_path(&self) -> SystemPath;

    /// Returns a reference to the system's deadletter box
    fn deadletter_ref(&self) -> ActorRef<Never>;
}

/// A trait to provide custom implementations of all system components
pub trait SystemComponents: Send + Sync {
    /// Return a reference to this deadletter box
    fn deadletter_ref(&self) -> ActorRef<Never>;
    /// Return a reference to this dispatcher
    fn dispatcher_ref(&self) -> DispatcherRef;
    /// Return a system path for this dispatcher
    fn system_path(&self) -> SystemPath;
    /// Start all the system components
    fn start(&self, _system: &KompactSystem) -> ();
    /// Stop all the system components
    fn stop(&self, _system: &KompactSystem) -> ();
}

/// Extra trait for timers to implement
pub trait TimerComponent: TimerRefFactory + Send + Sync {
    /// Stop the underlying timer thread
    fn shutdown(&self) -> Result<(), String>;
}

struct InternalComponents {
    supervisor: Arc<Component<ComponentSupervisor>>,
    supervision_port: ProvidedRef<SupervisionPort>,
    system_components: Box<dyn SystemComponents>,
}

impl InternalComponents {
    fn new(
        supervisor: Arc<Component<ComponentSupervisor>>,
        system_components: Box<dyn SystemComponents>,
    ) -> InternalComponents {
        let supervision_port = supervisor.on_definition(|s| s.supervision.share());
        InternalComponents {
            supervisor,
            supervision_port,
            system_components,
        }
    }

    fn start(&self, system: &KompactSystem) -> () {
        self.system_components.start(system);
        system.start(&self.supervisor);
    }

    fn deadletter_ref(&self) -> ActorRef<Never> {
        self.system_components.deadletter_ref()
    }

    fn dispatcher_ref(&self) -> DispatcherRef {
        self.system_components.dispatcher_ref()
    }

    fn system_path(&self) -> SystemPath {
        self.system_components.system_path()
    }

    fn supervision_port(&self) -> ProvidedRef<SupervisionPort> {
        self.supervision_port.clone()
    }

    fn stop(&self, system: &KompactSystem) -> () {
        let (p, f) = utils::promise();
        self.supervision_port.enqueue(SupervisorMsg::Shutdown(p));
        f.wait();
        self.system_components.stop(system);
    }
}

struct KompactRuntime {
    label: String,
    throughput: usize,
    max_messages: usize,
    timer: Box<dyn TimerComponent>,
    internal_components: OnceMutex<Option<InternalComponents>>,
    logger: KompactLogger,
    state: AtomicUsize,
}

impl KompactRuntime {
    fn new(conf: KompactConfig) -> Self {
        let mm = conf.max_messages();
        let logger = match conf.root_logger {
            Some(log) => log.new(o!("system" => conf.label.clone())),
            None => default_logger().new(o!("system" => conf.label.clone())),
        };
        KompactRuntime {
            label: conf.label,
            throughput: conf.throughput,
            max_messages: mm,
            timer: (conf.timer_builder)(),
            internal_components: OnceMutex::new(None),
            logger,
            state: lifecycle::initial_state(),
        }
    }

    fn set_internal_components(&self, internal_components: InternalComponents) -> () {
        let guard_opt: Option<OnceMutexGuard<'_, Option<InternalComponents>>> =
            self.internal_components.lock();
        if let Some(mut guard) = guard_opt {
            *guard = Some(internal_components);
        } else {
            panic!("KompactRuntime was already initialised!");
        }
    }

    fn start_internal_components(&self, system: &KompactSystem) -> () {
        match *self.internal_components {
            Some(ref ic) => {
                ic.start(system);
                lifecycle::set_active(self.state());
            }
            None => panic!("KompactRuntime was not properly initialised!"),
        }
    }

    fn logger(&self) -> &KompactLogger {
        &self.logger
    }

    /// Registers an actor with a path at the dispatcher
    fn register_by_path<D>(
        &self,
        actor_ref: &D,
        path: PathResolvable,
    ) -> Future<Result<ActorPath, RegistrationError>>
    where
        D: DynActorRefFactory,
    {
        debug!(self.logger(), "Requesting actor registration at {:?}", path);
        let (promise, future) = utils::promise();
        let dispatcher = self.dispatcher_ref();
        let envelope = MsgEnvelope::Typed(DispatchEnvelope::Registration(
            RegistrationEnvelope::with_promise(actor_ref, path, promise),
        ));
        dispatcher.enqueue(envelope);
        future
    }

    /// Registers an actor with an alias at the dispatcher
    fn register_by_alias<D>(
        &self,
        actor_ref: &D,
        alias: String,
    ) -> Future<Result<ActorPath, RegistrationError>>
    where
        D: DynActorRefFactory,
    {
        debug!(
            self.logger(),
            "Requesting actor alias registration for {:?}", alias
        );
        let path = PathResolvable::Alias(alias);
        self.register_by_path(actor_ref, path)
    }

    fn deadletter_ref(&self) -> ActorRef<Never> {
        match *self.internal_components {
            Some(ref sc) => sc.deadletter_ref(),
            None => panic!("KompactRuntime was not properly initialised!"),
        }
    }

    fn dispatcher_ref(&self) -> DispatcherRef {
        match *self.internal_components {
            Some(ref sc) => sc.dispatcher_ref(),
            None => panic!("KompactRuntime was not properly initialised!"),
        }
    }

    fn system_path(&self) -> SystemPath {
        match *self.internal_components {
            Some(ref sc) => sc.system_path(),
            None => panic!("KompactRuntime was not properly initialised!"),
        }
    }

    fn supervision_port(&self) -> ProvidedRef<SupervisionPort> {
        match *self.internal_components {
            Some(ref ic) => ic.supervision_port(),
            None => panic!("KompactRuntime was not properly initialised!"),
        }
    }

    fn timer_ref(&self) -> timer::TimerRef {
        self.timer.timer_ref()
    }

    fn shutdown(&self, system: &KompactSystem) -> Result<(), String> {
        match *self.internal_components {
            Some(ref ic) => {
                ic.stop(system);
            }
            None => panic!("KompactRuntime was not initialised at shutdown!"),
        }
        let res = self.timer.shutdown();
        lifecycle::set_destroyed(self.state());
        res
    }

    pub(crate) fn poison(&self) {
        lifecycle::set_faulty(self.state());
        let _ = self.timer.shutdown();
    }

    fn state(&self) -> &AtomicUsize {
        &self.state
    }

    fn is_active(&self) -> bool {
        lifecycle::is_active(self.state())
    }

    fn is_poisoned(&self) -> bool {
        lifecycle::is_faulty(self.state())
    }

    fn assert_active(&self) {
        assert!(self.is_active(), "KompactRuntime was not in active state!");
    }

    fn assert_not_poisoned(&self) {
        assert!(!self.is_poisoned(), "KompactRuntime was poisoned!");
    }
}

impl Debug for KompactRuntime {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(f, "KompactRuntime({})", self.label)
    }
}

/// API for a Kompact scheduler
///
/// Any scheduler implementation must implement this trait
/// so it can be used with Kompact.
///
/// Usually that means implementing some kind of wrapper
/// type for your particular scheduler, such as
/// [ExecutorScheduler](runtime::ExecutorScheduler), for example.
pub trait Scheduler: Send + Sync {
    /// Schedule `c` to be run on this scheduler
    ///
    /// Implementations must call [`c.execute()`](CoreContainer::execute)
    /// on the target thread.
    fn schedule(&self, c: Arc<dyn CoreContainer>) -> ();

    /// Shut this pool down asynchronously
    ///
    /// Implementations must eventually result in a correct
    /// shutdown, even when called from within one of its own threads.
    fn shutdown_async(&self) -> ();

    /// Shut this pool down synchronously
    ///
    /// Implementations must only return when the pool
    /// has been shut down, or upon an error.
    fn shutdown(&self) -> Result<(), String>;

    /// Clone an instance of this boxed
    ///
    /// Simply implement as `Box::new(self.clone())`.
    ///
    /// This is just a workaround for issues with boxed objects
    /// and [Clone](std::clone::Clone) implementations.
    fn box_clone(&self) -> Box<dyn Scheduler>;

    /// Handle the system being poisoned
    ///
    /// Usually this should just cause the scheduler to be
    /// shut down in an appropriate manner.
    fn poison(&self) -> ();
}

impl Clone for Box<dyn Scheduler> {
    fn clone(&self) -> Self {
        (*self).box_clone()
    }
}

/// A wrapper for schedulers from the [executors](executors) crate
#[derive(Clone)]
pub struct ExecutorScheduler<E>
where
    E: Executor + Sync,
{
    exec: E,
}

impl<E: Executor + Sync + 'static> ExecutorScheduler<E> {
    /// Produce a new `ExecutorScheduler` from an [Executor](executors::Executor) `E`.
    pub fn with(exec: E) -> ExecutorScheduler<E> {
        ExecutorScheduler { exec }
    }

    /// Produce a new boxed [Scheduler](runtime::Scheduler) from an [Executor](executors::Executor) `E`.
    pub fn from(exec: E) -> Box<dyn Scheduler> {
        Box::new(ExecutorScheduler::with(exec))
    }
}

impl<E: Executor + Sync + 'static> Scheduler for ExecutorScheduler<E> {
    fn schedule(&self, c: Arc<dyn CoreContainer>) -> () {
        self.exec.execute(move || {
            c.execute();
        });
    }

    fn shutdown_async(&self) -> () {
        self.exec.shutdown_async()
    }

    fn shutdown(&self) -> Result<(), String> {
        self.exec.shutdown_borrowed()
    }

    fn box_clone(&self) -> Box<dyn Scheduler> {
        Box::new(self.clone())
    }

    fn poison(&self) -> () {
        self.exec.shutdown_async();
    }
}

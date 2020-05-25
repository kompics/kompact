use super::*;

use crate::messaging::DispatchEnvelope;
use executors::*;
use std::{fmt, path::PathBuf, rc::Rc};

#[derive(Debug, Clone)]
pub(crate) enum ConfigSource {
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
    pub(crate) label: String,
    pub(crate) throughput: usize,
    pub(crate) msg_priority: f32,
    pub(crate) threads: usize,
    pub(crate) timer_builder: Rc<TimerBuilder>,
    pub(crate) scheduler_builder: Rc<SchedulerBuilder>,
    pub(crate) sc_builder: Rc<SCBuilder>,
    pub(crate) root_logger: Option<KompactLogger>,
    pub(crate) config_sources: Vec<ConfigSource>,
}

impl fmt::Debug for KompactConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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
            self.config_sources,
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
        assert!(
            n > 0,
            "The throughput must be larger than 0, or no events will be handled"
        );
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
        assert!(
            r > 0.0,
            "The msg_priority must be larger than 0.0, or no messages will be handled"
        );
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
        assert!(
            n > 0,
            "The number of threads must be more than 0, or no components will be run"
        );
        self.threads = n;
        self
    }

    /// Set a particular scheduler implementation
    ///
    /// Takes a function `f` from the number of threads to a concrete scheduler implementation as argument.
    pub fn scheduler<F>(&mut self, f: F) -> &mut Self
    where
        F: Fn(usize) -> Box<dyn Scheduler> + 'static,
    {
        self.scheduler_builder = Rc::new(f);
        self
    }

    /// Set a particular scheduler implementation based on an [executor](executors::core::Executor)
    ///
    /// Takes a function `f` from the number of threads to a concrete executor as argument.
    pub fn executor<E, F>(&mut self, f: F) -> &mut Self
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

    /// Set a particular set of system components
    ///
    /// This function works just like [system_components](KompactConfig::system_components),
    /// except that it assigns the dispatcher is pinned to its own thread using
    /// [create_dedicated_pinned_unsupervised](KompactSystem::create_dedicated_pinned_unsupervised).
    #[cfg(feature = "thread_pinning")]
    pub fn system_components_with_dedicated_dispatcher_pinned<B, C, FB, FC>(
        &mut self,
        deadletter_fn: FB,
        dispatcher_fn: FC,
        dispatcher_core: CoreId,
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
            let dispatcher = system
                .create_dedicated_pinned_unsupervised(|| dispatcher_fn(disp_prom), dispatcher_core);

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

    pub(crate) fn max_messages(&self) -> usize {
        let tpf = self.throughput as f32;
        let mmf = tpf * self.msg_priority;
        assert!(mmf >= 0.0, "msg_priority can not be negative!");
        mmf as usize
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
        let threads = std::cmp::max(1, num_cpus::get());
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

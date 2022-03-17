use super::*;

use crate::{
    config::{ConfigEntry, ConfigError, ConfigValueType, HoconExt},
    messaging::DispatchEnvelope,
};
use executors::*;
use hocon::Hocon;
use std::{fmt, path::PathBuf, rc::Rc};

/// Configuration keys for Kompact systems.
pub mod keys {
    use super::*;
    use crate::config::*;

    kompact_config! {
        LABEL,
        key = "kompact.runtime.label",
        type = StringValue,
        default = default_runtime_label(),
        doc = r#"The system label.

# Default

The default value is `kompact-runtime-` followed by a unique number (for this process).
    "#,
        version = "0.11"
    }

    kompact_config! {
        THROUGHPUT,
        key = "kompact.runtime.throughput",
        type = UsizeValue,
        default = 50,
        validate = |value| *value > 0,
        doc = r#"The scheduling granularity of a component.

This settings determines the maximum number of messages and events a component will handle before re-scheduling itself to cede compute resources to other components.
Thus, this settings allows users to tune fairness vs. throughput. 
Smaller values will give better fairness, while larger values will increase potential throughput.
The exact value depends on your workload per event. 
If each component does a lot compute for every event, then a low value will be sufficient to counter scheduling overhead.
If, however, the work per event is very low, performance will generally increase with higher values.

# Legal Values

Values must be `> 0`.

# Default

The default value is 50, which is tuned for relatively low-compute workloads (e.g., a few arithmetic operations per events and mayb a collection update/lookup).
    "#,
        version = "0.11"
    }

    kompact_config! {
        MESSAGE_PRIORITY,
        key = "kompact.runtime.message-priority",
        type = F32Value,
        default = 0.5,
        validate = |value| 0.0 < *value && *value < 1.0,
        doc = r#"The ratio between handling messages and events.

A component will handle up to *throughput × r* messages and *throughput × (1 - r)* events before re-scheduling.

If there are fewer events or messages than alloted queued up the remaining allotment will be redistributed to the other type until all throughput is used up or no messages or events remain.

# Legal Values

Values must be `> 0.0` and `< 1.0`.

Also note that this setting will not allow either type to be completely starved out.
Once multiplied with throughput a minimum of 1 message or event will be enforced.
This means you might not get the ratio you expect with low [throughput](crate::config_keys::system::THROUGHPUT) values.

# Default

The default value is 0.5, i.e. an even split.
    "#,
        version = "0.11"
    }

    kompact_config! {
        THREADS,
        key = "kompact.runtime.threads",
        type = UsizeValue,
        default = std::cmp::max(1, num_cpus::get()),
        validate = |value| *value > 0,
        doc = r#"The ratio between handling messages and events.

A component will handle up to *throughput × r* messages and *throughput × (1 - r)* events before re-scheduling.

If there are fewer events or messages than alloted queued up the remaining allotment will be redistributed to the other type until all throughput is used up or no messages or events remain.

# Legal Values

Values must be `> 0`.

# Default

The default value is 1 per cpu core or a minimum of 1.
    "#,
        version = "0.11"
    }

    kompact_config! {
        SCHEDULER,
        key = "kompact.runtime.scheduler",
        type = StringValue,
        default = "auto".to_string(),
        validate = |value| ["auto", "small", "large", "dynamic", "custom"].contains(&value.as_ref()),
        doc = r#"The scheduler implementation to use for the system.

A component will handle up to *throughput × r* messages and *throughput × (1 - r)* events before re-scheduling.

If there are fewer events or messages than alloted queued up the remaining allotment will be redistributed to the other type until all throughput is used up or no messages or events remain.

# Legal Values

- `auto`: Automatically pick the right implementation of [crossbeam_workstealing_pool](crossbeam_workstealing_pool)
- `small`: Use [small_pool](crossbeam_workstealing_pool::small_pool)
- `large`: Use [large_pool](crossbeam_workstealing_pool::large_pool)
- `dynamic`: Use [dyn_pool](crossbeam_workstealing_pool::dyn_pool)
- `custom`: Use the scheduler provided via [KompactConfig::scheduler](KompactConfig::scheduler) or [KompactConfig::executor](KompactConfig::executor) (this will be set automatically when you use those functions).

# Default

The default value is `auto`.
    "#,
        version = "0.11"
    }
}

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
    pub(crate) sc_builder: Rc<ScBuilder>,
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

/// A minimal Kompact configuration
///
/// - It the default label, i.e. `"kompact-runtime-{}"` for some sequence number.
/// - It sets the event/message throughput to 2, split evenly between events and messages.
/// - It runs with a single thread on a [small pool](crossbeam_workstealing_pool::small_pool).
pub const MINIMAL_CONFIG: &str = r#"
    kompact.system {
        throughput = 2
        message-priority = 0.5
        threads = 1
    }
"#;

impl KompactConfig {
    /// Create a minimal Kompact config
    ///
    /// - The minimal config uses the default label, i.e. `"kompact-runtime-{}"` for some sequence number.
    /// - It sets the event/message throughput to 2, split evenly between events and messages.
    /// - It runs with a single thread on a [small pool](crossbeam_workstealing_pool::small_pool).
    /// - It uses all default components, without networking, with the default timer and default logger.
    #[deprecated(
        since = "0.11.0",
        note = "If you really want these exact parameters, use `KompactConfig::load_config_str(kompact::runtime::MINIMAL_CONFIG)` instead. Otherwise prefer `KompactConfig::default()`."
    )]
    pub fn new() -> KompactConfig {
        KompactConfig {
            label: "<unset>".to_string(),
            throughput: 0,
            msg_priority: 0.0,
            threads: 1,
            timer_builder: Rc::new(DefaultTimer::new_timer_component),
            scheduler_builder: Rc::new(|t| {
                ExecutorScheduler::from(crossbeam_workstealing_pool::small_pool(t))
            }),
            sc_builder: Rc::new(|sys, dead_prom, disp_prom| {
                Box::new(DefaultComponents::new(sys, dead_prom, disp_prom))
            }),
            root_logger: None,
            config_sources: vec![ConfigSource::Str(MINIMAL_CONFIG.to_string())],
        }
    }

    /// Set the name of the system
    ///
    /// The label is used as metadata for logging output.
    #[deprecated(
        since = "0.11.0",
        note = "Use `KompactConfig::set_config_value(&kompact::config_keys::system::LABEL, ...)` instead."
    )]
    pub fn label<I>(&mut self, s: I) -> &mut Self
    where
        I: Into<String>,
    {
        let v: String = s.into();
        self.set_config_value(&keys::LABEL, v);
        self
    }

    /// Set the maximum number of events/messages to handle before rescheduling a component
    ///
    /// Larger values can increase throughput on highly loaded components,
    /// but at the cost of fairness between components.
    #[deprecated(
        since = "0.11.0",
        note = "Use `KompactConfig::set_config_value(&kompact::config_keys::system::THROUGHPUT, ...)` instead."
    )]
    pub fn throughput(&mut self, n: usize) -> &mut Self {
        self.set_config_value(&keys::THROUGHPUT, n);
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
    #[deprecated(
        since = "0.11.0",
        note = "Use `KompactConfig::set_config_value(&kompact::config_keys::system::MESSAGE_PRIORITY, ...)` instead."
    )]
    pub fn msg_priority(&mut self, r: f32) -> &mut Self {
        self.set_config_value(&keys::MESSAGE_PRIORITY, r);
        self
    }

    /// The number of threads in the Kompact thread pool
    ///
    /// # Note
    ///
    /// You *must* ensure that the selected [scheduler](KompactConfig::scheduler) implementation
    /// can manage the given number of threads, if you customise this value!
    #[deprecated(
        since = "0.11.0",
        note = "Use `KompactConfig::set_config_value(&kompact::config_keys::system::THREADS, ...)` instead."
    )]
    pub fn threads(&mut self, n: usize) -> &mut Self {
        assert!(
            n > 0,
            "The number of threads must be more than 0, or no components will be run"
        );
        self.set_config_value(&keys::THREADS, n);
        self
    }

    /// Set a particular scheduler implementation
    ///
    /// Takes a function `f` from the number of threads to a concrete scheduler implementation as argument.
    pub fn scheduler<F>(&mut self, f: F) -> &mut Self
    where
        F: Fn(usize) -> Box<dyn Scheduler> + 'static,
    {
        self.set_config_value(&keys::SCHEDULER, "custom".to_string());
        self.scheduler_builder = Rc::new(f);
        self
    }

    /// Set a particular scheduler implementation based on a [FuturesExecutor](executors::FuturesExecutor)
    ///
    /// Takes a function `f` from the number of threads to a concrete executor as argument.
    pub fn executor<E, F>(&mut self, f: F) -> &mut Self
    where
        E: FuturesExecutor + Sync + 'static,
        F: Fn(usize) -> E + 'static,
    {
        self.set_config_value(&keys::SCHEDULER, "custom".to_string());
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
        FB: Fn(KPromise<()>) -> B + 'static,
        FC: Fn(KPromise<()>) -> C + 'static,
    {
        let sb = move |system: &KompactSystem, dead_prom: KPromise<()>, disp_prom: KPromise<()>| {
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
        FB: Fn(KPromise<()>) -> B + 'static,
        FC: Fn(KPromise<()>) -> C + 'static,
    {
        let sb = move |system: &KompactSystem, dead_prom: KPromise<()>, disp_prom: KPromise<()>| {
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
        FB: Fn(KPromise<()>) -> B + 'static,
        FC: Fn(KPromise<()>) -> C + 'static,
    {
        let sb = move |system: &KompactSystem, dead_prom: KPromise<()>, disp_prom: KPromise<()>| {
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
    ///
    /// It matters in which order configs are loaded and values are set.
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
    ///
    /// It matters in which order configs are loaded and values are set.
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

    /// Override a single value in the HOCON config
    ///
    /// This method can be called multiple times and the resulting configurations will be merged.
    ///
    /// It matters in which order configs are loaded and values are set.
    /// See [HoconLoader](hocon::HoconLoader) for more information.
    pub fn set_config_value<T>(
        &mut self,
        config: &ConfigEntry<T>,
        value: <T as ConfigValueType>::Value,
    ) -> &mut Self
    where
        T: ConfigValueType,
    {
        let value_string = <T as ConfigValueType>::config_string(value);
        self.config_sources.push(ConfigSource::Str(format!(
            "{} = {}",
            config.key, value_string
        )));
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

    pub(crate) fn override_from_hocon(&mut self, conf: &Hocon) -> Result<(), ConfigError> {
        self.label = conf.get_or_default(&keys::LABEL)?;
        self.throughput = conf.get_or_default(&keys::THROUGHPUT)?;
        self.msg_priority = conf.get_or_default(&keys::MESSAGE_PRIORITY)?;
        self.threads = conf.get_or_default(&keys::THREADS)?;
        println!("THREADSSSS {}", self.threads);
        let scheduler_option = conf.get_or_default(&keys::SCHEDULER)?;
        match scheduler_option.as_ref() {
            "auto" => {
                self.scheduler_builder = if self.threads <= 32 {
                    Rc::new(|t| ExecutorScheduler::from(crossbeam_workstealing_pool::small_pool(t)))
                } else if self.threads <= 64 {
                    Rc::new(|t| ExecutorScheduler::from(crossbeam_workstealing_pool::large_pool(t)))
                } else {
                    Rc::new(|t| ExecutorScheduler::from(crossbeam_workstealing_pool::dyn_pool(t)))
                };
            }
            "small" => {
                self.scheduler_builder = Rc::new(|t| {
                    ExecutorScheduler::from(crossbeam_workstealing_pool::small_pool(t))
                });
            }
            "large" => {
                self.scheduler_builder = Rc::new(|t| {
                    ExecutorScheduler::from(crossbeam_workstealing_pool::large_pool(t))
                });
            }
            "dynamic" => {
                self.scheduler_builder =
                    Rc::new(|t| ExecutorScheduler::from(crossbeam_workstealing_pool::dyn_pool(t)))
            }
            "custom" => (), // ignore, since the value is already set
            _ => unreachable!(
                "Options should be checked by the validator! {} is illegal.",
                scheduler_option
            ),
        }

        Ok(())
    }
}

impl Default for KompactConfig {
    /// Create a default Kompact config
    ///
    /// # Defaults
    ///
    /// See the following configuration keys for the default values:
    ///
    /// - [LABEL](crate::config_keys::system::LABEL)
    /// - [THROUGHPUT](crate::config_keys::system::THROUGHPUT)
    /// - [MESSAGE_PRIORITY](crate::config_keys::system::MESSAGE_PRIORITY)
    /// - [THREADS](crate::config_keys::system::THREADS)
    ///
    /// It uses all default components, without networking, with the default timer and default logger.
    fn default() -> Self {
        // NOTE: Most of the values we are setting in here don't actually matter
        // They will be overwritten by the default values of the config keys in `KompactConfig::override_from_hocon`
        let scheduler_builder: Rc<SchedulerBuilder> =
            Rc::new(|t| ExecutorScheduler::from(crossbeam_workstealing_pool::small_pool(t)));
        KompactConfig {
            label: "<unset>".to_string(),
            throughput: 0,
            msg_priority: 0.0,
            threads: 1,
            timer_builder: Rc::new(DefaultTimer::new_timer_component),
            scheduler_builder,
            sc_builder: Rc::new(|sys, dead_prom, disp_prom| {
                Box::new(DefaultComponents::new(sys, dead_prom, disp_prom))
            }),
            root_logger: None,
            config_sources: Vec::new(),
        }
    }
}

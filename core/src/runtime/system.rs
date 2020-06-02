use super::*;

#[cfg(all(nightly, feature = "type_erasure"))]
use crate::utils::erased::ErasedComponentDefinition;
use crate::{
    messaging::{
        DispatchEnvelope,
        MsgEnvelope,
        PathResolvable,
        RegistrationEnvelope,
        RegistrationId,
        RegistrationResponse,
        RegistrationResult,
    },
    supervision::{ComponentSupervisor, ListenEvent, SupervisionPort, SupervisorMsg},
    timer::timer_manager::TimerRefFactory,
};
use hocon::{Hocon, HoconLoader};
use oncemutex::{OnceMutex, OnceMutexGuard};
use std::{fmt, sync::Mutex};

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
    fn load_config(conf: &KompactConfig) -> Result<Hocon, KompactError> {
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
        Ok(config)
    }

    /// Use the [build](KompactConfig::build) method instead.
    pub(crate) fn try_new(conf: KompactConfig) -> Result<Self, KompactError> {
        let scheduler = (*conf.scheduler_builder)(conf.threads);
        let sc_builder = conf.sc_builder.clone();

        let config = Self::load_config(&conf)?;
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
        c
    }

    /// Create a new component from type-erased definition
    ///
    /// Since components are shared between threads, the created component
    /// is internally wrapped into an [Arc](std::sync::Arc).
    ///
    /// Newly created components are not started automatically.
    /// Use [start](KompactSystem::start) or
    /// [start_notify](KompactSystem::start_notify) to start a newly
    /// created component, once it is connected properly.
    ///
    /// If you need address this component via the network, see the [register](KompactSystem::register) function.
    ///
    /// # Example
    ///
    /// ```
    /// # use kompact::prelude::*;
    /// # use kompact::doctest_helpers::*;
    /// # let system = KompactConfig::default().build().expect("system");
    /// let c = system.create_erased(Box::new(TestComponent1::new()));
    /// # system.shutdown().expect("shutdown");
    /// ```
    #[cfg(all(nightly, feature = "type_erasure"))]
    #[inline(always)]
    pub fn create_erased<M: MessageBounds>(
        &self,
        a: Box<dyn ErasedComponentDefinition<M>>,
    ) -> Arc<dyn AbstractComponent<Message = M>> {
        a.spawn_on(self)
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
        c
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
        promise.fulfil(c.clone()).expect("Should accept component");
        c
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
        promise.fulfil(c.clone()).expect("Should accept component");
        c
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
        promise.fulfil(c.clone()).expect("Should accept component");
        c
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
        promise.fulfil(c.clone()).expect("Should accept component");
        c
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
    pub fn register(&self, c: &Arc<impl AbstractComponent + ?Sized>) -> Future<RegistrationResult> {
        self.inner.assert_active();
        let id = c.core().id().clone();
        let id_path = PathResolvable::ActorId(id);
        self.inner.register_by_path(c.as_ref(), false, id_path) // never update unique registrations
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
    pub fn create_and_register<C, F>(&self, f: F) -> (Arc<Component<C>>, Future<RegistrationResult>)
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

    /// Attempts to register the provided component with a human-readable alias.
    ///
    /// The returned future will contain the named [ActorPath](ActorPath)
    /// for the given alias, once it is completed by the dispatcher.
    ///
    /// Alias registration will fail if a previous registration already exists.
    /// Use [update_alias_registration](KompactSystem::update_alias_registration) to override an existing registration.
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
    pub fn register_by_alias<A>(
        &self,
        c: &Arc<impl AbstractComponent + ?Sized>,
        alias: A,
    ) -> Future<RegistrationResult>
    where
        A: Into<String>,
    {
        self.inner.assert_active();
        self.inner
            .register_by_alias(c.as_ref(), false, alias.into())
    }

    /// Attempts to register the provided component with a human-readable alias.
    ///
    /// The returned future will contain the named [ActorPath](ActorPath)
    /// for the given alias, once it is completed by the dispatcher.
    ///
    /// This registration will replace any previous registration, if it exists.
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
    /// let alias_registration_future = system.update_alias_registration(&c, "test");
    /// alias_registration_future.wait_expect(Duration::from_millis(1000), "Failed to register TestComponent1 by alias");
    /// let alias_reregistration_future = system.update_alias_registration(&c, "test");
    /// alias_reregistration_future.wait_expect(Duration::from_millis(1000), "Failed to override TestComponent1 registration by alias");
    /// # system.shutdown().expect("shutdown");
    /// ```
    pub fn update_alias_registration<A>(
        &self,
        c: &Arc<impl AbstractComponent + ?Sized>,
        alias: A,
    ) -> Future<RegistrationResult>
    where
        A: Into<String>,
    {
        self.inner.assert_active();
        self.inner.register_by_alias(c.as_ref(), true, alias.into())
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
    pub fn start(&self, c: &Arc<impl AbstractComponent + ?Sized>) -> () {
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
    pub fn start_notify(&self, c: &Arc<impl AbstractComponent + ?Sized>) -> Future<()> {
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
    pub fn stop(&self, c: &Arc<impl AbstractComponent + ?Sized>) -> () {
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
    pub fn stop_notify(&self, c: &Arc<impl AbstractComponent + ?Sized>) -> Future<()> {
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
    pub fn kill(&self, c: Arc<impl AbstractComponent + ?Sized>) -> () {
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
    pub fn kill_notify(&self, c: Arc<impl AbstractComponent + ?Sized>) -> Future<()> {
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
    pub fn actor_path_for(&self, component: &Arc<impl AbstractComponent + ?Sized>) -> ActorPath {
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

    /// Create a new component from type-erased definition
    ///
    /// Since components are shared between threads, the created component
    /// is internally wrapped into an [Arc](std::sync::Arc).
    ///
    /// Newly created components are not started automatically.
    /// Use [start](KompactSystem::start) or
    /// [start_notify](KompactSystem::start_notify) to start a newly
    /// created component, once it is connected properly.
    ///
    /// If you need address this component via the network, see the [register](KompactSystem::register) function.
    ///
    /// # Example
    ///
    /// ```
    /// # use kompact::prelude::*;
    /// # use kompact::doctest_helpers::*;
    /// # let system = KompactConfig::default().build().expect("system");
    /// let c = system.create_erased(Box::new(TestComponent1::new()));
    /// # system.shutdown().expect("shutdown");
    /// ```
    #[cfg(all(nightly, feature = "type_erasure"))]
    fn create_erased<M: MessageBounds>(
        &self,
        a: Box<dyn ErasedComponentDefinition<M>>,
    ) -> Arc<dyn AbstractComponent<Message = M>>;

    /// Attempts to register `c` with the dispatcher using its unique id
    ///
    /// The returned id can be used to match a later response message
    /// which will contain the unique id [ActorPath](ActorPath)
    /// for the given component, once it is completed by the dispatcher.
    ///
    /// Once the response message is received, the component can be addressed via the network,
    /// even if it has not been started, yet (in which case messages will simply be queued up).
    ///
    /// # Example
    ///
    /// ```
    /// use kompact::prelude::*;
    /// # use kompact::doctest_helpers::*;
    /// use std::time::Duration;
    /// use std::sync::Arc;
    ///
    /// #[derive(ComponentDefinition)]
    /// struct ParentComponent {
    ///     ctx: ComponentContext<Self>,
    ///     child: Option<Arc<Component<TestComponent1>>>,
    ///     reg_id: Option<RegistrationId>,
    /// }
    /// impl ParentComponent {
    ///     fn new() -> Self {
    ///         ParentComponent {
    ///             ctx: ComponentContext::new(),
    ///             child: None,
    ///             reg_id: None,
    ///         }
    ///     }
    /// }
    /// impl Provide<ControlPort> for ParentComponent {
    ///    fn handle(&mut self, event: ControlEvent) -> () {
    ///         match event {
    ///             ControlEvent::Start => {
    ///                 let child = self.ctx.system().create(TestComponent1::new);
    ///                 let id = self.ctx.system().register(&child, self);
    ///                 self.reg_id = Some(id);
    ///                 self.child = Some(child);
    ///             }
    ///             ControlEvent::Stop | ControlEvent::Kill => {
    ///                 let _ = self.child.take(); // don't hang on to the child
    ///             }
    ///         }
    ///     }
    /// }
    /// impl Actor for ParentComponent {
    ///     type Message = RegistrationResponse;
    ///
    ///     fn receive_local(&mut self, msg: Self::Message) -> () {
    ///         assert_eq!(msg.id, self.reg_id.take().unwrap());
    ///         info!(self.log(), "Child was registered");
    ///         if let Some(ref child) = self.child {
    ///             self.ctx.system().start(child);
    ///             let path = msg.result.expect("actor path");
    ///             path.tell((), self); // can send it messages via its path now
    ///         } else {
    ///             unreachable!("Wouldn't have asked for registration without storing the child");
    ///         }
    ///     }
    ///
    ///     fn receive_network(&mut self, _msg: NetMessage) -> () {
    ///         unimplemented!("unused");
    ///     }
    /// }
    ///
    /// let mut cfg = KompactConfig::new();
    /// cfg.system_components(DeadletterBox::new, NetworkConfig::default().build());
    /// let system = cfg.build().expect("KompactSystem");
    /// let parent = system.create(ParentComponent::new);
    /// # std::thread::sleep(Duration::from_millis(1000));
    /// # system.shutdown().expect("shutdown");
    /// ```
    fn register(
        &self,
        c: &Arc<impl AbstractComponent + ?Sized>,
        reply_to: &dyn Receiver<RegistrationResponse>,
    ) -> RegistrationId;

    /// Attempts to register `c` with the dispatcher using its unique id
    ///
    /// Same as [register](SystemHandle::register) except requesting that no response be send.
    fn register_without_response(&self, c: &Arc<impl AbstractComponent + ?Sized>) -> ();

    /// Attempts to register the provided component with a human-readable alias
    ///
    /// The returned id can be used to match a later response message
    /// which will contain the named [ActorPath](ActorPath)
    /// for the given alias, once it is completed by the dispatcher.
    ///
    /// Alias registration will fail if a previous registration already exists.
    /// Use [update_alias_registration](SystemHandle::update_alias_registration) to override an existing registration.
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
    /// use std::sync::Arc;
    ///
    /// #[derive(ComponentDefinition)]
    /// struct ParentComponent {
    ///     ctx: ComponentContext<Self>,
    ///     child: Option<Arc<Component<TestComponent1>>>,
    ///     reg_id: Option<RegistrationId>,
    /// }
    /// impl ParentComponent {
    ///     fn new() -> Self {
    ///         ParentComponent {
    ///             ctx: ComponentContext::new(),
    ///             child: None,
    ///             reg_id: None,
    ///         }
    ///     }
    /// }
    /// impl Provide<ControlPort> for ParentComponent {
    ///    fn handle(&mut self, event: ControlEvent) -> () {
    ///         match event {
    ///             ControlEvent::Start => {
    ///                 let child = self.ctx.system().create(TestComponent1::new);
    ///                 let id = self.ctx.system().register_by_alias(&child, "test", self);
    ///                 self.reg_id = Some(id);
    ///                 self.child = Some(child);
    ///             }
    ///             ControlEvent::Stop | ControlEvent::Kill => {
    ///                 let _ = self.child.take(); // don't hang on to the child
    ///             }
    ///         }
    ///     }
    /// }
    /// impl Actor for ParentComponent {
    ///     type Message = RegistrationResponse;
    ///
    ///     fn receive_local(&mut self, msg: Self::Message) -> () {
    ///         assert_eq!(msg.id, self.reg_id.take().unwrap());
    ///         info!(self.log(), "Child was registered");
    ///         if let Some(ref child) = self.child {
    ///             self.ctx.system().start(child);
    ///             let path = msg.result.expect("actor path");
    ///             path.tell((), self); // can send it messages via its path now
    ///         } else {
    ///             unreachable!("Wouldn't have asked for registration without storing the child");
    ///         }
    ///     }
    ///
    ///     fn receive_network(&mut self, _msg: NetMessage) -> () {
    ///         unimplemented!("unused");
    ///     }
    /// }
    ///
    /// let mut cfg = KompactConfig::new();
    /// cfg.system_components(DeadletterBox::new, NetworkConfig::default().build());
    /// let system = cfg.build().expect("KompactSystem");
    /// let parent = system.create(ParentComponent::new);
    /// # std::thread::sleep(Duration::from_millis(1000));
    /// # system.shutdown().expect("shutdown");
    /// ```
    fn register_by_alias<A>(
        &self,
        c: &Arc<impl AbstractComponent + ?Sized>,
        alias: A,
        reply_to: &dyn Receiver<RegistrationResponse>,
    ) -> RegistrationId
    where
        A: Into<String>;

    /// Attempts to register the provided component with a human-readable alias
    ///
    /// Same as [register_by_alias](SystemHandle::register_by_alias) except requesting that no response be send.
    fn register_by_alias_without_response<A>(
        &self,
        c: &Arc<impl AbstractComponent + ?Sized>,
        alias: A,
    ) -> ()
    where
        A: Into<String>;

    /// Attempts to register the provided component with a human-readable alias.
    ///
    /// The returned id can be used to match a later response message
    /// which will contain the named [ActorPath](ActorPath)
    /// for the given alias, once it is completed by the dispatcher.
    ///
    /// This registration will replace any previous registration, if it exists.
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
    /// use std::sync::Arc;
    ///
    /// #[derive(ComponentDefinition)]
    /// struct ParentComponent {
    ///     ctx: ComponentContext<Self>,
    ///     child: Option<Arc<Component<TestComponent1>>>,
    ///     reg_id: Option<RegistrationId>,
    /// }
    /// impl ParentComponent {
    ///     fn new() -> Self {
    ///         ParentComponent {
    ///             ctx: ComponentContext::new(),
    ///             child: None,
    ///             reg_id: None,
    ///         }
    ///     }
    /// }
    /// impl Provide<ControlPort> for ParentComponent {
    ///    fn handle(&mut self, event: ControlEvent) -> () {
    ///         match event {
    ///             ControlEvent::Start => {
    ///                 let child = self.ctx.system().create(TestComponent1::new);
    ///                 let id = self.ctx.system().update_alias_registration(&child, "test", self);
    ///                 self.reg_id = Some(id);
    ///                 self.child = Some(child);
    ///             }
    ///             ControlEvent::Stop | ControlEvent::Kill => {
    ///                 let _ = self.child.take(); // don't hang on to the child
    ///             }
    ///         }
    ///     }
    /// }
    /// impl Actor for ParentComponent {
    ///     type Message = RegistrationResponse;
    ///
    ///     fn receive_local(&mut self, msg: Self::Message) -> () {
    ///         assert_eq!(msg.id, self.reg_id.take().unwrap());
    ///         info!(self.log(), "Child was registered");
    ///         if let Some(ref child) = self.child {
    ///             self.ctx.system().start(child);
    ///             let path = msg.result.expect("actor path");
    ///             path.tell((), self); // can send it messages via its path now
    ///         } else {
    ///             unreachable!("Wouldn't have asked for registration without storing the child");
    ///         }
    ///     }
    ///
    ///     fn receive_network(&mut self, _msg: NetMessage) -> () {
    ///         unimplemented!("unused");
    ///     }
    /// }
    ///
    /// let mut cfg = KompactConfig::new();
    /// cfg.system_components(DeadletterBox::new, NetworkConfig::default().build());
    /// let system = cfg.build().expect("KompactSystem");
    /// let parent = system.create(ParentComponent::new);
    /// # std::thread::sleep(Duration::from_millis(1000));
    /// # system.shutdown().expect("shutdown");
    /// ```
    fn update_alias_registration<A>(
        &self,
        c: &Arc<impl AbstractComponent + ?Sized>,
        alias: A,
        reply_to: &dyn Receiver<RegistrationResponse>,
    ) -> RegistrationId
    where
        A: Into<String>;

    /// Attempts to register the provided component with a human-readable alias
    ///
    /// Same as [update_alias_registration](SystemHandle::update_alias_registration) except requesting that no response be send.
    fn update_alias_registration_without_response<A>(
        &self,
        c: &Arc<impl AbstractComponent + ?Sized>,
        alias: A,
    ) -> ()
    where
        A: Into<String>;

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
    fn start(&self, c: &Arc<impl AbstractComponent + ?Sized>) -> ();

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
    fn stop(&self, c: &Arc<impl AbstractComponent + ?Sized>) -> ();

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
    fn kill(&self, c: Arc<impl AbstractComponent + ?Sized>) -> ();

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
        update: bool,
        path: PathResolvable,
    ) -> Future<RegistrationResult>
    where
        D: DynActorRefFactory + ?Sized,
    {
        debug!(self.logger(), "Requesting actor registration at {:?}", path);
        let (promise, future) = utils::promise();
        let dispatcher = self.dispatcher_ref();
        let envelope = MsgEnvelope::Typed(DispatchEnvelope::Registration(
            RegistrationEnvelope::with_promise(actor_ref, path, update, promise),
        ));
        dispatcher.enqueue(envelope);
        future
    }

    /// Registers an actor with an alias at the dispatcher
    fn register_by_alias<D>(
        &self,
        actor_ref: &D,
        update: bool,
        alias: String,
    ) -> Future<RegistrationResult>
    where
        D: DynActorRefFactory + ?Sized,
    {
        debug!(
            self.logger(),
            "Requesting actor alias registration for {:?}", alias
        );
        let path = PathResolvable::Alias(alias);
        self.register_by_path(actor_ref, update, path)
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

impl fmt::Debug for KompactRuntime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "KompactRuntime({})", self.label)
    }
}

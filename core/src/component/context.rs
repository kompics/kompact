use super::*;

use crate::net::buffers::{BufferConfig, ChunkAllocator, ChunkRef};
use std::task::Poll;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StateTransition {
    Active,
    Passive,
    Destroyed,
}
impl Default for StateTransition {
    fn default() -> Self {
        StateTransition::Active
    }
}

#[derive(Debug)]
struct BlockingState {
    future: BlockingFuture,
    unblock_state: StateTransition,
}

/// The contextual object for a Kompact component
///
/// Gives access compact internal features like
/// timers, logging, confguration, an the self reference.
pub struct ComponentContext<CD: ComponentTraits> {
    inner: Option<ComponentContextInner<CD>>,
    buffer: RefCell<Option<EncodeBuffer>>,
    blocking_future: Option<BlockingState>,
    pub(super) non_blocking_futures: FxHashMap<Uuid, NonBlockingFuture>,
}

struct ComponentContextInner<CD: ComponentTraits> {
    timer_manager: TimerManager<CD>,
    pub(super) component: Weak<Component<CD>>,
    logger: KompactLogger,
    actor_ref: ActorRef<CD::Message>,
    config: Arc<Hocon>,
    id: Uuid,
}

impl<CD> ComponentContext<CD>
where
    CD: ComponentTraits + ComponentLifecycle,
{
    /// Create a new, uninitialised component context
    ///
    /// # Note
    ///
    /// Nothing in this context may be used *before* the parent component is actually initialised!
    pub fn uninitialised() -> ComponentContext<CD> {
        ComponentContext {
            inner: None,
            buffer: RefCell::new(None),
            blocking_future: None,
            non_blocking_futures: FxHashMap::default(),
        }
    }

    /// Initialise the component context with the actual component instance
    ///
    /// This *must* be invoked from [setup](ComponentDefinition::setup).
    pub fn initialise(&mut self, c: Arc<Component<CD>>) -> () {
        let system = c.system();
        let id = c.id();
        let inner = ComponentContextInner {
            timer_manager: TimerManager::new(system.timer_ref()),
            component: Arc::downgrade(&c),
            logger: c.logger().new(o!("ctype" => CD::type_name())),
            actor_ref: c.actor_ref(),
            config: system.config_owned(),
            id,
        };
        self.inner = Some(inner);
        trace!(self.log(), "Initialised.");
    }

    fn inner_ref(&self) -> &ComponentContextInner<CD> {
        match self.inner {
            Some(ref c) => c,
            None => panic!("Component improperly initialised!"),
        }
    }

    fn inner_mut(&mut self) -> &mut ComponentContextInner<CD> {
        match self.inner {
            Some(ref mut c) => c,
            None => panic!("Component improperly initialised!"),
        }
    }

    /// The components logger instance
    ///
    /// This instance will already be preloaded
    /// with component specific information in the MDC.
    ///
    /// # Example
    ///
    /// ```
    /// use kompact::prelude::*;
    ///
    /// #[derive(ComponentDefinition, Actor)]
    /// struct HelloLogging {
    ///    ctx: ComponentContext<Self>,
    /// }
    /// impl HelloLogging {
    ///     fn new() -> HelloLogging {
    ///         HelloLogging {
    ///             ctx: ComponentContext::uninitialised(),
    ///         }
    ///     }    
    /// }
    /// impl ComponentLifecycle for HelloLogging {
    ///     fn on_start(&mut self) -> Handled {
    ///         info!(self.ctx().log(), "Hello Start event");
    ///         self.ctx().system().shutdown_async();
    ///         Handled::Ok
    ///     }    
    /// }
    ///
    /// let system = KompactConfig::default().build().expect("system");
    /// let c = system.create(HelloLogging::new);
    /// system.start(&c);
    /// system.await_termination();
    /// ```
    pub fn log(&self) -> &KompactLogger {
        &self.inner_ref().logger
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
    ///
    /// #[derive(ComponentDefinition, Actor)]
    /// struct ConfigComponent {
    ///    ctx: ComponentContext<Self>,
    /// }
    /// impl ConfigComponent {
    ///     fn new() -> ConfigComponent {
    ///         ConfigComponent {
    ///             ctx: ComponentContext::uninitialised(),
    ///         }
    ///     }    
    /// }
    /// impl ComponentLifecycle for ConfigComponent {
    ///     fn on_start(&mut self) -> Handled {
    ///         assert_eq!(Some(7i64), self.ctx().config()["a"].as_i64());
    ///         self.ctx().system().shutdown_async();
    ///         Handled::Ok
    ///     }    
    /// }
    /// let default_values = r#"{ a = 7 }"#;
    /// let mut conf = KompactConfig::default();
    /// conf.load_config_str(default_values);
    /// let system = conf.build().expect("system");
    /// let c = system.create(ConfigComponent::new);
    /// system.start(&c);
    /// system.await_termination();
    /// ```
    pub fn config(&self) -> &Hocon {
        self.inner_ref().config.as_ref()
    }

    pub(crate) fn timer_manager_mut(&mut self) -> &mut TimerManager<CD> {
        &mut self.inner_mut().timer_manager
    }

    pub(super) fn set_blocking_with_state(
        &mut self,
        future: BlockingFuture,
        unblock_state: StateTransition,
    ) {
        let blocking_state = BlockingState {
            future,
            unblock_state,
        };
        #[cfg(nightly)]
        {
            self.blocking_future
                .replace(blocking_state)
                .expect_none("Replacing a blocking future without completing it first is invalid!");
        }
        #[cfg(not(nightly))]
        {
            assert!(
                self.blocking_future.replace(blocking_state).is_none(),
                "Replacing a blocking future without completing it first is invalid!"
            );
        }
        let component = self.typed_component();
        component.set_blocking();
    }

    /// Sets the component to block on the provided blocking `future`
    ///
    /// This should *only* be used when implementing custom [execute](ComponentDefinition::execute) logic!
    /// Otherwise the correct way to block is to return the [Handled::BlockOn](Handled::BlockOn) variant from a handler.
    ///
    /// If this is used for custom [execute](ComponentDefinition::execute) logic, then the next call
    /// should be a `return ExecuteResult::new(true, count, skip)`, as continuing to execute handlers violates
    /// blocking semantics.
    pub fn set_blocking(&mut self, future: BlockingFuture) {
        self.set_blocking_with_state(future, StateTransition::default())
    }

    /// Return `true` if the component is set up for blocking
    ///
    /// If this does return `true`, port handling **must** be immediately
    /// aborted and the returned result must have the form
    /// `ExecuteResult::new(true, count, skip)`.
    pub fn is_blocking(&self) -> bool {
        self.blocking_future.is_some()
    }

    pub(super) fn run_blocking_task(&mut self) -> SchedulingDecision {
        let mut blocking_state = self
            .blocking_future
            .take()
            .expect("Called run_blocking while not blocking!");
        let component = self.typed_component();
        match blocking_state.future.run(&component) {
            BlockingRunResult::BlockOn(f) => {
                blocking_state.future = f;
                #[cfg(nightly)]
                {
                    self.blocking_future.replace(blocking_state).expect_none(
                        "Replacing a blocking future without completing it first is invalid!",
                    );
                }
                #[cfg(not(nightly))]
                {
                    assert!(
                        self.blocking_future.replace(blocking_state).is_none(),
                        "Replacing a blocking future without completing it first is invalid!"
                    );
                }
                SchedulingDecision::Blocked
            }
            BlockingRunResult::Unblock => {
                assert!(
                    self.blocking_future.is_none(),
                    "Don't block within a blocking future! Just call await on the future instead."
                );
                match blocking_state.unblock_state {
                    StateTransition::Active => component.set_active(),
                    StateTransition::Passive => component.set_passive(),
                    StateTransition::Destroyed => component.set_destroyed(),
                }
                SchedulingDecision::Resume
            }
        }
    }

    pub(super) fn run_nonblocking_task(&mut self, tag: Uuid) -> Handled {
        if let Some(future) = self.non_blocking_futures.get_mut(&tag) {
            match future.run() {
                Poll::Pending => Handled::Ok,
                Poll::Ready(handled) => {
                    self.non_blocking_futures.remove(&tag);
                    handled
                }
            }
        } else {
            warn!(self.log(), "Future with tag {} was scheduled but not available. May have been scheduled after completion.", tag);
            Handled::Ok
        }
    }

    pub(crate) fn context_system(&self) -> ContextSystemHandle {
        ContextSystemHandle::from(self.component())
    }

    /// Returns the component instance wrapping this component definition
    ///
    /// This is mostly meant to be passed along for scheduling or registrations.
    /// Don't try to lock anything on the thread already executing the component!
    pub fn typed_component(&self) -> Arc<Component<CD>> {
        match self.inner_ref().component.upgrade() {
            Some(ac) => ac,
            None => panic!("Component already deallocated!"),
        }
    }

    /// Returns the component instance wrapping this component definition
    ///
    /// This is mostly meant to be passed along for scheduling and the like.
    /// Don't try to lock anything on the thread already executing the component!
    pub fn component(&self) -> Arc<dyn CoreContainer> {
        self.typed_component()
    }

    /// Returns a handle to the Kompact system this component is a part of
    pub fn system(&self) -> impl SystemHandle {
        self.context_system()
    }

    /// Returns a reference to the system dispatcher
    pub fn dispatcher_ref(&self) -> DispatcherRef {
        self.system().dispatcher_ref()
    }

    /// Returns a reference to the system's deadletter box
    pub fn deadletter_ref(&self) -> ActorRef<Never> {
        self.system().deadletter_ref()
    }

    /// Returns a reference to this components unique id
    pub fn id(&self) -> &Uuid {
        &self.inner_ref().id
    }

    /// Destroys this component lazily
    ///
    /// This simply sends a `Kill` event to itself,
    /// which means that other events may still be handled
    /// before the component is actually killed.
    ///
    /// For a more immediate alternative
    /// see [Handled::DieNow](Handled::DieNow).
    pub fn suicide(&self) -> () {
        self.component().enqueue_control(ControlEvent::Kill);
    }

    pub(crate) fn with_buffer<R>(&self, f: impl FnOnce(&mut EncodeBuffer) -> R) -> R {
        {
            // Scoping the borrow
            if let Some(buffer) = self.buffer.borrow_mut().as_mut() {
                return f(buffer);
            }
        }
        self.init_buffers(None, None);
        self.with_buffer(f)
    }

    /// Attempts to create a [ChunkRef](net::buffers::ChunkRef) of the data using the local
    /// [EncodeBuffer](net::buffers::EncodeBuffer), to be used with
    /// [tell_preserialised](actors::ActorPath#method.tell_preserialised)
    pub fn preserialise<B>(&self, content: &B) -> Result<ChunkRef, SerError>
    where
        B: Serialisable + Sized,
    {
        {
            // Scoping the borrow
            if let Some(buffer) = self.buffer.borrow_mut().as_mut() {
                return crate::ser_helpers::preserialise_msg(
                    content,
                    &mut buffer.get_buffer_encoder(),
                );
            }
        }
        self.init_buffers(None, None);
        self.preserialise(content)
    }

    /// May be used for manual initialization of a components local
    /// [EncodeBuffer](net::buffers::EncodeBuffer). If this method is never called explicitly
    /// the actor will implicitly call it when it tries to serialise its first message.
    ///
    /// If buffers have already been initialized, explicitly or implicitly, the method does nothing.
    ///
    /// A custom [BufferConfig](net::buffers::BufferConfig) may be specified, if `None` is given the
    /// actor will first try to parse the configuration from the system wide `Hocon`-Configuration,
    /// if that fails, the default config will be used.
    ///
    /// A custom [ChunkAllocator](net::buffers::ChunkAllocator) may also be specified.
    /// if `None` is given the actor will use the default allocator.
    ///
    /// Note: The `BufferConfig` within the systems [NetworkConfig](dispatch::NetworkConfig) never
    /// affects the Actors Buffers (i.e. this method).
    pub fn init_buffers(
        &self,
        buffer_config: Option<BufferConfig>,
        custom_allocator: Option<Arc<dyn ChunkAllocator>>,
    ) -> () {
        let mut buffer_location = self.buffer.borrow_mut();

        if buffer_location.as_mut().is_none() {
            // Need to create a new Buffer, fetch BufferConfig
            let cfg = {
                if let Some(cfg) = buffer_config {
                    cfg
                } else {
                    self.get_buffer_config()
                }
            };

            if custom_allocator.is_some() {
                debug!(
                    self.log(),
                    "init_buffers with custom allocator, config {:?}.", &cfg
                );
            } else {
                debug!(
                    self.log(),
                    "init_buffers with default allocator, config {:?}.", &cfg
                );
            }

            let buffer =
                EncodeBuffer::with_dispatcher_ref(self.dispatcher_ref(), &cfg, custom_allocator);
            *buffer_location = Some(buffer);
        }
    }

    /// Returns a BufferConfig from the global configuration or the default configuration.
    fn get_buffer_config(&self) -> BufferConfig {
        BufferConfig::from_config(self.config())
    }

    /// We use this method for assertions in tests
    #[allow(dead_code)]
    pub(crate) fn get_buffer_location(&self) -> &RefCell<Option<EncodeBuffer>> {
        &self.buffer
    }

    /// Set the recovery function for this component
    ///
    /// See [RecoveryHandler](crate::prelude::RecoveryHandler) for more information.
    ///
    /// You can perform action repeatedly in order to store state
    /// to use while recovering from a failure.
    /// Do note, however, that this call causes an additional mutex lock
    /// which is not necessarily cheap and can theoretically deadlock,
    /// if you call this from more than one place.
    pub fn set_recovery_function<F>(&self, f: F) -> ()
    where
        F: FnOnce(FaultContext) -> RecoveryHandler + Send + 'static,
    {
        self.typed_component().set_recovery_function(f);
    }
}

impl<CD> ActorRefFactory for ComponentContext<CD>
where
    CD: ComponentTraits + ComponentLifecycle,
{
    type Message = CD::Message;

    fn actor_ref(&self) -> ActorRef<CD::Message> {
        self.inner_ref().actor_ref.clone()
    }
}

impl<CD> ActorPathFactory for ComponentContext<CD>
where
    CD: ComponentTraits + ComponentLifecycle,
{
    fn actor_path(&self) -> ActorPath {
        let id = *self.id();
        ActorPath::Unique(UniquePath::with_system(self.system().system_path(), id))
    }
}

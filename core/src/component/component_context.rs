use super::*;

/// The contextual object for a Kompact component
///
/// Gives access compact internal features like
/// timers, logging, confguration, an the self reference.
pub struct ComponentContext<CD: ComponentDefinition + Sized + 'static> {
    inner: Option<ComponentContextInner<CD>>,
}

struct ComponentContextInner<CD: ComponentDefinition + ActorRaw + Sized + 'static> {
    timer_manager: TimerManager<CD>,
    pub(super) component: Weak<Component<CD>>,
    logger: KompactLogger,
    actor_ref: ActorRef<CD::Message>,
    buffer: RefCell<Option<EncodeBuffer>>,
    config: Arc<Hocon>,
    id: Uuid,
    blocking_future: Option<BlockingFuture>,
}

impl<CD: ComponentDefinition + Sized + 'static> ComponentContext<CD> {
    /// Create a new, uninitialised component context
    ///
    /// # Note
    ///
    /// Nothing in this context may be used *before* the parent component is actually initialised!
    pub fn uninitialised() -> ComponentContext<CD> {
        ComponentContext { inner: None }
    }

    /// Initialise the component context with the actual component instance
    ///
    /// This *must* be invoked from [setup](ComponentDefinition::setup).
    pub fn initialise(&mut self, c: Arc<Component<CD>>) -> ()
    where
        CD: ComponentDefinition + 'static,
    {
        let system = c.system();
        let id = *c.id();
        let inner = ComponentContextInner {
            timer_manager: TimerManager::new(system.timer_ref()),
            component: Arc::downgrade(&c),
            logger: c.logger().new(o!("ctype" => CD::type_name())),
            actor_ref: c.actor_ref(),
            buffer: RefCell::new(None),
            config: system.config_owned(),
            id,
            blocking_future: None,
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
    /// impl Provide<ControlPort> for HelloLogging {
    ///     fn handle(&mut self, event: ControlEvent) -> Handled {
    ///         info!(self.ctx().log(), "Hello control event: {:?}", event);
    ///         if event == ControlEvent::Start {
    ///             self.ctx().system().shutdown_async();
    ///         }
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
    /// impl Provide<ControlPort> for ConfigComponent {
    ///     fn handle(&mut self, event: ControlEvent) -> Handled {
    ///         match event {
    ///             ControlEvent::Start => {
    ///                 assert_eq!(Some(7i64), self.ctx().config()["a"].as_i64());
    ///                 self.ctx().system().shutdown_async();
    ///             }
    ///             _ => (), // ignore
    ///         }
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

    pub(crate) fn typed_component(&self) -> Arc<Component<CD>> {
        match self.inner_ref().component.upgrade() {
            Some(ac) => ac,
            None => panic!("Component already deallocated!"),
        }
    }

    /// Sets the component to block on the provided blocking future `f`
    ///
    /// This should *only* be used when implementing custom [execute](ComponentDefinition::execute) logic!
    /// Otherwise the correct way to block is to return the [Handled::BlockOn](Handled::BlockOn) variant from a handler.
    ///
    /// If this is used for custom [execute](ComponentDefinition::execute) logic, then the next call
    /// should be a `return ExecuteResult::new(true, count, skip)`, as continuing to execute handlers violates
    /// blocking semantics.
    pub fn set_blocking(&mut self, f: BlockingFuture) {
        let inner_mut = self.inner_mut();
        inner_mut
            .blocking_future
            .replace(f)
            .expect_none("Replacing a blocking future without completing it first is invalid!");
        let component = match inner_mut.component.upgrade() {
            Some(ac) => ac,
            None => panic!("Component already deallocated!"),
        };
        component.set_blocking();
    }

    /// Return `true` if the component is set up for blocking
    ///
    /// If this does return `true`, port handling **must** be immediately
    /// aborted and the returned result must have the form
    /// `ExecuteResult::new(true, count, skip)`.
    pub fn is_blocking(&self) -> bool {
        self.inner_ref().blocking_future.is_some()
    }

    pub(super) fn run_blocking_task(&mut self) -> SchedulingDecision {
        let blocking_future;
        let component;
        {
            let inner_mut = self.inner_mut();
            blocking_future = inner_mut
                .blocking_future
                .take()
                .expect("Called run_blocking while not blocking!");
            component = match inner_mut.component.upgrade() {
                Some(ac) => ac,
                None => panic!("Component already deallocated!"),
            };
            // let inner_mut go out of scope here, as it would illegal to hold a mutable reference to it
            // while running the blocking_future, which also gets a mutable reference
        }
        match blocking_future.run(&component) {
            BlockingRunResult::BlockOn(f) => {
                let inner_mut = self.inner_mut();
                inner_mut.blocking_future.replace(f).expect_none(
                    "Replacing a blocking future without completing it first is invalid!",
                );
                SchedulingDecision::Blocked
            }
            BlockingRunResult::Unblock => {
                let inner_mut = self.inner_mut();
                assert!(
                    inner_mut.blocking_future.is_none(),
                    "Don't block within a blocking future! Just call await on the future instead."
                );
                component.set_active();
                SchedulingDecision::NoWork
            }
        }
    }

    /// Returns the component instance wrapping this component definition
    ///
    /// This is mostly meant to be passed along for scheduling and the like.
    /// Don't try to lock anything on the already thread executing the component!
    pub fn component(&self) -> Arc<dyn CoreContainer> {
        self.typed_component()
    }

    /// Returns a handle to the Kompact system this component is a part of
    pub fn system(&self) -> impl SystemHandle {
        ContextSystemHandle::from(self.component())
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

    /// Destroys this component
    ///
    /// Simply sends a [Kill](ControlEvent::Kill) event to itself.
    pub fn suicide(&self) -> () {
        self.component().control_port().enqueue(ControlEvent::Kill);
    }

    /// Initializes a buffer pool which [tell_serialised(ActorPath::tell_serialised) can use.
    pub fn initialise_pool(&self) -> () {
        debug!(self.log(), "Initialising EncodeBuffer");
        *self.inner_ref().buffer.borrow_mut() = Some(EncodeBuffer::new());
    }

    /// Get a reference to the interior EncodeBuffer without retaining a self borrow
    /// initializes the private pool if it has not already been initialized
    pub fn get_buffer(&self) -> &RefCell<Option<EncodeBuffer>> {
        &self.inner_ref().buffer
    }
}

impl<CD> ActorRefFactory for ComponentContext<CD>
where
    CD: ComponentDefinition + ActorRaw + Sized + 'static,
{
    type Message = CD::Message;

    fn actor_ref(&self) -> ActorRef<CD::Message> {
        self.inner_ref().actor_ref.clone()
    }
}

impl<CD> ActorPathFactory for ComponentContext<CD>
where
    CD: ComponentDefinition + ActorRaw + Sized + 'static,
{
    fn actor_path(&self) -> ActorPath {
        let id = *self.id();
        ActorPath::Unique(UniquePath::with_system(self.system().system_path(), id))
    }
}

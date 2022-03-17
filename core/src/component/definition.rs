use super::*;

/// Object-safe part of [`ComponentDefinition`].
///
/// This trait aggregates all the object-safe super-traits of [`ComponentDefinition`] to make
/// trait objects possible while rust doesn't have multi-trait trait-objects.
pub trait DynamicComponentDefinition:
    DynamicPortAccess + ActorRaw + ComponentLifecycle + Send
{
}

impl<T> DynamicComponentDefinition for T where
    T: DynamicPortAccess + ActorRaw + ComponentLifecycle + Send
{
}

/// The core trait every component must implement
///
/// Should usually simply be derived using `#[derive(ComponentDefinition)]`.
///
/// Only implement this manually if you need special execution logic,
/// for example for custom fairness models.
///
/// # Note
///
/// The derive macro additionally provides implementation of
/// [ProvideRef](ProvideRef) or [RequireRef](RequireRef) for each of the
/// component's ports. It is generally recommended to do so as well, when not
/// using the derive macro, as it enables some rather convenient APIs.
pub trait ComponentDefinition: DynamicComponentDefinition
where
    Self: Sized + 'static,
{
    /// Prepare the component for being run
    ///
    /// You *must* call [initialise](ComponentContext::initialise) on this
    /// component's context instance.
    ///
    /// You *must* call [set_parent](ProvidedPort::set_parent) (or [RequiredPort::set_parent](RequiredPort::set_parent))
    /// for each of the component's ports.
    fn setup(&mut self, self_component: Arc<Component<Self>>) -> ();

    /// Execute events on the component's ports
    ///
    /// You may run up to `max_events` events from the component's ports.
    ///
    /// The `skip` value normally contains the offset where the last invocation stopped.
    /// However, you can specify the next value when you create the returning [ExecuteResult](ExecuteResult),
    /// so you can custome the semantics of this value, if desired.
    fn execute(&mut self, max_events: usize, skip: usize) -> ExecuteResult;

    /// Return a reference the component's context field
    fn ctx(&self) -> &ComponentContext<Self>;

    /// Return a mutable reference the component's context field
    fn ctx_mut(&mut self) -> &mut ComponentContext<Self>;

    /// Return the name of the component's type
    ///
    /// This is only used for the logging MDC, so you can technically
    /// return whatever you like. It simply helps with debugging if it's related
    /// to the actual struct name.
    fn type_name() -> &'static str;

    /// Run a Future on this component, allowing it mutable
    /// access to the component's internal state on every poll.
    ///
    /// Please see the documentation for [ComponentDefinitionAccess](ComponentDefinitionAccess)
    /// for details on how the internal state may (and may not) be used.
    ///
    /// # Example
    ///
    /// ```
    /// # use kompact::prelude::*;
    ///
    /// #[derive(ComponentDefinition, Actor)]
    /// struct AsyncComponent {
    ///    ctx: ComponentContext<Self>,
    ///    flag: bool,
    /// }
    /// impl AsyncComponent {
    ///     fn new() -> Self {
    ///         AsyncComponent {
    ///             ctx: ComponentContext::uninitialised(),
    ///             flag: false,    
    ///         }
    ///     }   
    /// }
    /// impl ComponentLifecycle for AsyncComponent {
    ///     fn on_start(&mut self) -> Handled {
    ///         // on nightly you can just write: async move |mut async_self| {...}
    ///         self.spawn_local(move |mut async_self| async move {
    ///             async_self.flag = true;
    ///             Handled::Ok
    ///         });
    ///         Handled::Ok
    ///     }   
    /// }
    /// ```
    ///
    /// # See Also
    ///
    /// In order to suspend processing of all other messages and events while completing a
    /// future, use [block_on](Handled::block_on).
    ///
    /// In order to run a large future which does not need access to component's internal state
    /// at all or until the very end, consider using [spawn_off](ComponentDefinition::spawn_off).
    fn spawn_local<F>(&mut self, f: impl FnOnce(ComponentDefinitionAccess<Self>) -> F)
    where
        Self: 'static,
        F: futures::Future<Output = Handled> + Send + 'static,
    {
        let future = future_task::non_blocking(self, f);
        println!("AAAAAAAAA Future");
        future.schedule();
        let tag = future.tag();
        self.ctx_mut().non_blocking_futures.insert(tag, future);
    }

    /// Run a Future on this system's executor pool and return a handle to the result
    ///
    /// Handles can be awaited like any other future.
    ///
    /// # Note
    ///
    /// The current API is not as efficient as calling [FuturesExecutor::spawn](executors::FuturesExecutor::spawn)
    /// directly, due to some trait object indirection in Kompact systems.
    /// Thus, if performance is important, it is recommended to maintain a (non trait-object) handle
    /// to the actual `Executor` pool being used and call its `spawn` function instead.
    /// This API is really just a somewhat roundabout convenience for doing the same.
    fn spawn_off<R: Send + 'static>(
        &self,
        future: impl futures::Future<Output = R> + 'static + Send,
    ) -> JoinHandle<R> {
        self.ctx().system().spawn(future)
    }
}

/// A trait to customise handling of lifecycle events
///
/// This trait replaces the pre-v0.9 `Provide<ControlPort>` requirement.
pub trait ComponentLifecycle: ComponentLogging {
    /// Gets invoked every time a component receives a Start event
    ///
    /// The default implementation simply logs something at debug level.
    fn on_start(&mut self) -> Handled
    where
        Self: 'static,
    {
        debug!(self.log(), "Starting...");
        Handled::Ok
    }

    /// Gets invoked every time a component receives a Stop event
    ///
    /// The default implementation simply logs something at debug level.
    fn on_stop(&mut self) -> Handled
    where
        Self: 'static,
    {
        debug!(self.log(), "Stopping...");
        Handled::Ok
    }

    /// Gets invoked every time a component receives a Kill event
    ///
    /// The default implementation simply logs something at debug level.
    fn on_kill(&mut self) -> Handled
    where
        Self: 'static,
    {
        debug!(self.log(), "Killing...");
        Handled::Ok
    }
}

/// A mechanism for dynamically getting references to provided/required ports from a component.
///
/// Should only be used if working with concrete types, or strict generic bounds (i.e. [`Provide`],
/// [`ProvideRef`], etc.) is not an option. This is the case, for example, when working with
/// `Arc<dyn `[`AbstractComponent`]`>`.
pub trait DynamicPortAccess {
    /// **Internal API**. Dynamically obtain a mutable reference to a [`ProvidedPort`] if `self`
    /// provides a port of the type indicated by the passed `port_id`.
    ///
    /// This is a low-level API that is automatically implemented by
    /// `#[derive(ComponentDefinition)]`. Prefer the more strongly typed
    /// [`get_provided_port`](trait.DynamicComponentDefinition.html#method.get_provided_port).
    fn get_provided_port_as_any(&mut self, port_id: std::any::TypeId) -> Option<&mut dyn Any>;

    /// **Internal API**. Dynamically obtain a mutable reference to a [`RequiredPort`] if `self`
    /// requires a port of the type indicated by the passed `port_id`.
    ///
    /// This is a low-level API that is automatically implemented by
    /// `#[derive(ComponentDefinition)]`. Prefer the more strongly typed
    /// [`get_required_port`](trait.DynamicComponentDefinition.html#method.get_required_port).
    fn get_required_port_as_any(&mut self, port_id: std::any::TypeId) -> Option<&mut dyn Any>;
}

impl<'a, M: MessageBounds> dyn DynamicComponentDefinition<Message = M> + 'a {
    /// Dynamically obtain a mutable reference to a [`ProvidedPort<P>`](ProvidedPort) if `self`
    /// provides a port of type `P`.
    pub fn get_provided_port<P: Port>(&mut self) -> Option<&mut ProvidedPort<P>> {
        self.get_provided_port_as_any(std::any::TypeId::of::<P>())
            .and_then(|any| any.downcast_mut())
    }

    /// Dynamically obtain a mutable reference to a [`RequiredPort<P>`](RequiredPort) if `self`
    /// requires a port of type `P`.
    pub fn get_required_port<P: Port>(&mut self) -> Option<&mut RequiredPort<P>> {
        self.get_required_port_as_any(std::any::TypeId::of::<P>())
            .and_then(|any| any.downcast_mut())
    }
}

/// Mutex guard guarding a [`DynamicComponentDefinition`] trait object.
pub type DynamicComponentDefinitionMutexGuard<'a, M> =
    OwningRefMut<Box<dyn Erased + 'a>, dyn DynamicComponentDefinition<Message = M>>;

/// Error for when the component definition lock has been poisoned
#[derive(Debug)]
pub struct LockPoisoned;
impl fmt::Display for LockPoisoned {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "component definition lock has been poisoned")
    }
}
impl std::error::Error for LockPoisoned {}

/// An object-safe trait that exposes most of functionality of a [`Component`] that isn't
/// dependent on a particular [`ComponentDefinition`].
///
/// Useful if you want to reduce code bloat by removing the generic parameter from `Component<CD>`.
///
/// See also: [`ActorRefFactory`] and [`CoreContainer`], which this trait inherits.
pub trait AbstractComponent: MsgQueueContainer + CoreContainer + Any {
    /// Message type of this component
    //type Message: MessageBounds;

    /// Returns a mutable reference to the underlying component definition as a
    /// [`DynamicComponentDefinition`] trait object.
    ///
    /// This can only be done if you have a reference to the component instance that isn't hidden
    /// behind an [Arc](std::sync::Arc). For example, after the system shuts down and your code
    /// holds on to the last reference to a component you can use [get_mut](std::sync::Arc::get_mut)
    /// or [try_unwrap](std::sync::Arc::try_unwrap).
    fn dyn_definition_mut(
        &mut self,
    ) -> &mut dyn DynamicComponentDefinition<Message = Self::Message>;

    /// Locks the component definition mutex and returns a guard that can be dereferenced to
    /// access a [`DynamicComponentDefinition`] trait object.
    fn lock_dyn_definition(
        &self,
    ) -> Result<DynamicComponentDefinitionMutexGuard<Self::Message>, LockPoisoned>;

    /// Views self as [`Any`](std::any::Any). Can be used to downcast to a concrete [`Component`].
    fn as_any(&self) -> &dyn Any;

    /// Upcast this
    ///
    /// Needed to implement [ActorRefFactory](ActorRefFactory) due to
    /// <https://github.com/rust-lang/rust/issues/18469>
    fn as_queue_container(self: Arc<Self>) -> Weak<dyn MsgQueueContainer<Message = Self::Message>>;
}

impl<C> AbstractComponent for Component<C>
where
    C: ComponentTraits + ComponentLifecycle,
{
    //type Message = C::Message;

    fn dyn_definition_mut(
        &mut self,
    ) -> &mut dyn DynamicComponentDefinition<Message = Self::Message> {
        self.definition_mut()
    }

    fn lock_dyn_definition(
        &self,
    ) -> Result<DynamicComponentDefinitionMutexGuard<Self::Message>, LockPoisoned> {
        let lock = self.mutable_core.lock().map_err(|_| LockPoisoned)?;
        let res = OwningRefMut::new(Box::new(lock))
            .map_mut(|l| {
                (&mut l.deref_mut().definition)
                    as &mut dyn DynamicComponentDefinition<Message = Self::Message>
            })
            .erase_owner();
        Ok(res)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_queue_container(self: Arc<Self>) -> Weak<dyn MsgQueueContainer<Message = Self::Message>> {
        let res: Weak<Self> = Arc::downgrade(&self);
        res as Weak<dyn MsgQueueContainer<Message = Self::Message>>
    }
}

impl<M: MessageBounds> dyn AbstractComponent<Message = M> {
    /// Execute a function on the underlying component definition
    /// and return the result. The component definition will be accessed as a
    /// [`DynamicComponentDefinition`] trait object.
    ///
    /// This method will attempt to lock the mutex, and then apply `f` to the component definition
    /// inside the guard.
    ///
    /// This method wraps the mutex guard in an additional allocation. Prefer
    /// [`Component::on_definition`] where possible.
    pub fn on_dyn_definition<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut dyn DynamicComponentDefinition<Message = M>) -> R,
    {
        let mut lock = self.lock_dyn_definition().unwrap();
        f(&mut *lock)
    }
}

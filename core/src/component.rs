use std::{
    fmt,
    ops::DerefMut,
    panic,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
        Mutex,
        Weak,
    },
    time::Duration,
};
use uuid::Uuid;
//use oncemutex::OnceMutex;
use hocon::Hocon;
use std::cell::{RefCell, UnsafeCell};

use super::*;
use crate::{
    actors::TypedMsgQueue,
    messaging::{
        DispatchEnvelope,
        MsgEnvelope,
        PathResolvable,
        RegistrationEnvelope,
        RegistrationId,
        RegistrationResponse,
    },
    supervision::*,
    timer::timer_manager::{ExecuteAction, ScheduledTimer, Timer, TimerManager, TimerRefFactory},
};

use crate::net::buffer::EncodeBuffer;
#[cfg(all(nightly, feature = "type_erasure"))]
use crate::utils::erased::CreateErased;
use owning_ref::{Erased, OwningRefMut};
use std::any::Any;

/// A trait for abstracting over structures that contain a component core
///
/// Used for implementing scheduling and execution logic,
/// such as [Scheduler](runtime::Scheduler).
pub trait CoreContainer: Send + Sync {
    /// Returns the component's unique id
    fn id(&self) -> &Uuid;
    /// Returns a reference to the actual component core
    fn core(&self) -> &ComponentCore;
    /// Executes this component on the current thread
    fn execute(&self) -> ();
    /// Returns a reference to this component's control port
    fn control_port(&self) -> ProvidedRef<ControlPort>;
    /// Returns this component's system
    fn system(&self) -> &KompactSystem {
        self.core().system()
    }
    /// Schedules this component on its associated scheduler
    fn schedule(&self) -> ();
    /// The descriptive string of the [ComponentDefinition](ComponentDefinition) type wrapped in this container
    fn type_name(&self) -> &'static str;
}

impl fmt::Debug for dyn CoreContainer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CoreContainer({})", self.id())
    }
}

/// A concrete component instance
///
/// The component class itself is application agnostic,
/// but it contains the application specific [ComponentDefinition](ComponentDefinition).
pub struct Component<C: ComponentDefinition + ActorRaw + Sized + 'static> {
    core: ComponentCore,
    custom_scheduler: Option<dedicated_scheduler::DedicatedThreadScheduler>,
    definition: Mutex<C>,
    ctrl_queue: Arc<ConcurrentQueue<<ControlPort as Port>::Request>>,
    msg_queue: Arc<TypedMsgQueue<C::Message>>,
    skip: AtomicUsize,
    state: AtomicUsize,
    supervisor: Option<ProvidedRef<SupervisionPort>>,
    // system components don't have supervision
    logger: KompactLogger,
}

impl<C: ComponentDefinition + Sized> Component<C> {
    pub(crate) fn new(
        system: KompactSystem,
        definition: C,
        supervisor: ProvidedRef<SupervisionPort>,
    ) -> Component<C> {
        let core = ComponentCore::with::<Component<C>>(system);
        let logger = core
            .system
            .logger()
            .new(o!("cid" => format!("{}", core.id)));
        let msg_queue = Arc::new(TypedMsgQueue::new());
        Component {
            core,
            custom_scheduler: None,
            definition: Mutex::new(definition),
            ctrl_queue: Arc::new(ConcurrentQueue::new()),
            msg_queue,
            skip: AtomicUsize::new(0),
            state: lifecycle::initial_state(),
            supervisor: Some(supervisor),
            logger,
        }
    }

    pub(crate) fn with_dedicated_scheduler(
        system: KompactSystem,
        definition: C,
        supervisor: ProvidedRef<SupervisionPort>,
        custom_scheduler: dedicated_scheduler::DedicatedThreadScheduler,
    ) -> Component<C> {
        let core = ComponentCore::with::<Component<C>>(system);
        let logger = core
            .system
            .logger()
            .new(o!("cid" => format!("{}", core.id)));
        let msg_queue = Arc::new(TypedMsgQueue::new());
        Component {
            core,
            custom_scheduler: Some(custom_scheduler),
            definition: Mutex::new(definition),
            ctrl_queue: Arc::new(ConcurrentQueue::new()),
            msg_queue,
            skip: AtomicUsize::new(0),
            state: lifecycle::initial_state(),
            supervisor: Some(supervisor),
            logger,
        }
    }

    pub(crate) fn without_supervisor(system: KompactSystem, definition: C) -> Component<C> {
        let core = ComponentCore::with::<Component<C>>(system);
        let logger = core
            .system
            .logger()
            .new(o!("cid" => format!("{}", core.id)));
        Component {
            core,
            custom_scheduler: None,
            definition: Mutex::new(definition),
            ctrl_queue: Arc::new(ConcurrentQueue::new()),
            msg_queue: Arc::new(TypedMsgQueue::new()),
            skip: AtomicUsize::new(0),
            state: lifecycle::initial_state(),
            supervisor: None,
            logger,
        }
    }

    pub(crate) fn without_supervisor_with_dedicated_scheduler(
        system: KompactSystem,
        definition: C,
        custom_scheduler: dedicated_scheduler::DedicatedThreadScheduler,
    ) -> Component<C> {
        let core = ComponentCore::with::<Component<C>>(system);
        let logger = core
            .system
            .logger()
            .new(o!("cid" => format!("{}", core.id)));
        Component {
            core,
            custom_scheduler: Some(custom_scheduler),
            definition: Mutex::new(definition),
            ctrl_queue: Arc::new(ConcurrentQueue::new()),
            msg_queue: Arc::new(TypedMsgQueue::new()),
            skip: AtomicUsize::new(0),
            state: lifecycle::initial_state(),
            supervisor: None,
            logger,
        }
    }

    // pub(crate) fn msg_queue_wref(&self) -> Weak<ConcurrentQueue<MsgEnvelope>> {
    //     Arc::downgrade(&self.msg_queue)
    // }

    pub(crate) fn enqueue_control(&self, event: <ControlPort as Port>::Request) -> () {
        self.ctrl_queue.push(event);
        match self.core.increment_work() {
            SchedulingDecision::Schedule => {
                self.schedule();
            }
            _ => (), // nothing
        }
    }

    /// Returns the mutex containing the underlying [ComponentDefinition](ComponentDefinition).
    pub fn definition(&self) -> &Mutex<C> {
        &self.definition
    }

    /// Returns a mutable reference to the underlying component definition.
    ///
    /// This can only be done if you have a reference to the component instance
    /// that isn't hidden behind an [Arc](std::sync::Arc). For example,
    /// after the system shuts down and your code holds on to the last reference to a component
    /// you can use [get_mut](std::sync::Arc::get_mut) or [try_unwrap](std::sync::Arc::try_unwrap).
    pub fn definition_mut(&mut self) -> &mut C {
        self.definition.get_mut().unwrap()
    }

    /// Execute a function on the underlying [ComponentDefinition](ComponentDefinition)
    /// and return the result
    ///
    /// This method will attempt to lock the mutex, and then apply `f` to the component definition
    /// inside the guard.
    pub fn on_definition<T, F>(&self, f: F) -> T
    where
        F: FnOnce(&mut C) -> T,
    {
        let mut cd = self.definition.lock().unwrap();
        f(cd.deref_mut())
    }

    /// Returns a reference to this component's logger
    pub fn logger(&self) -> &KompactLogger {
        &self.logger
    }

    /// Returns `true` if the component is marked as *faulty*.
    pub fn is_faulty(&self) -> bool {
        lifecycle::is_faulty(&self.state)
    }

    /// Returns `true` if the component is marked as *active*.
    pub fn is_active(&self) -> bool {
        lifecycle::is_active(&self.state)
    }

    /// Returns `true` if the component is marked as *destroyed*.
    pub fn is_destroyed(&self) -> bool {
        lifecycle::is_destroyed(&self.state)
    }

    /// Wait synchronously for this component be either *destroyed* or *faulty*
    ///
    /// This component blocks the current thread and hot-waits for the component
    /// to become either *faulty* or *destroyed*. It is meant mostly for testing
    /// and not recommended in production.
    pub fn wait_ended(&self) -> () {
        loop {
            if self.is_faulty() || self.is_destroyed() {
                return;
            }
        }
    }

    fn inner_execute(&self) {
        let max_events = self.core.system.throughput();
        let max_messages = self.core.system.max_messages();
        match self.definition().lock() {
            Ok(mut guard) => {
                let mut count: usize = 0;
                while let Ok(event) = self.ctrl_queue.pop() {
                    // ignore max_events for lifecyle events
                    // println!("Executing event: {:?}", event);
                    let supervisor_msg = match event {
                        lifecycle::ControlEvent::Start => {
                            lifecycle::set_active(&self.state);
                            debug!(self.logger, "Component started.");
                            SupervisorMsg::Started(self.core.component())
                        }
                        lifecycle::ControlEvent::Stop => {
                            lifecycle::set_passive(&self.state);
                            debug!(self.logger, "Component stopped.");
                            SupervisorMsg::Stopped(self.core.id)
                        }
                        lifecycle::ControlEvent::Kill => {
                            lifecycle::set_destroyed(&self.state);
                            debug!(self.logger, "Component killed.");
                            SupervisorMsg::Killed(self.core.id)
                        }
                    };
                    guard.handle(event);
                    count += 1;
                    // inform supervisor after local handling to make sure crashing component don't count as started
                    if let Some(ref supervisor) = self.supervisor {
                        supervisor.enqueue(supervisor_msg);
                    }
                }
                if (!lifecycle::is_active(&self.state)) {
                    trace!(self.logger, "Not running inactive scheduled.");
                    match self.core.decrement_work(count) {
                        SchedulingDecision::Schedule => self.schedule(),
                        _ => (), // ignore
                    }
                    return;
                }
                // timers have highest priority
                while count < max_events {
                    let c = guard.deref_mut();
                    match c.ctx_mut().timer_manager_mut().try_action() {
                        ExecuteAction::Once(id, action) => {
                            action(c, id);
                            count += 1;
                        }
                        ExecuteAction::Periodic(id, action) => {
                            action(c, id);
                            count += 1;
                        }
                        ExecuteAction::None => break,
                    }
                }
                // then some messages
                while count < max_messages {
                    if let Some(env) = self.msg_queue.pop() {
                        guard.receive(env);
                        count += 1;
                    } else {
                        break;
                    }
                }
                // then events
                let rem_events = max_events.saturating_sub(count);
                if (rem_events > 0) {
                    let res = guard.execute(rem_events, self.skip.load(Ordering::Relaxed));
                    self.skip.store(res.skip, Ordering::Relaxed);
                    count = count + res.count;

                    // and maybe some more messages
                    while count < max_events {
                        if let Some(env) = self.msg_queue.pop() {
                            guard.receive(env);
                            count += 1;
                        } else {
                            break;
                        }
                    }
                }
                match self.core.decrement_work(count) {
                    SchedulingDecision::Schedule => self.schedule(),
                    _ => (), // ignore
                }
            }
            _ => {
                panic!("Component {} is poisoned but not faulty!", self.id());
            }
        }
    }
}

impl<CD> ActorRefFactory for Arc<Component<CD>>
where
    CD: ComponentDefinition + ActorRaw + 'static,
{
    type Message = CD::Message;

    fn actor_ref(&self) -> ActorRef<CD::Message> {
        let comp = Arc::downgrade(self);
        let msgq = Arc::downgrade(&self.msg_queue);
        ActorRef::new(comp, msgq)
    }
}

// impl<CD> DynActorRefFactory for Arc<Component<CD>>
// where
//     CD: ComponentDefinition + ActorRaw + 'static,
// {
//     fn dyn_ref(&self) -> DynActorRef {
//         let component = Arc::downgrade(self);
//         let msg_queue = Arc::downgrade(&self.msg_queue);
//         DynActorRef::from(component, msg_queue)
//     }
// }

impl<CD> ActorRefFactory for CD
where
    CD: ComponentDefinition + ActorRaw + 'static,
{
    type Message = CD::Message;

    fn actor_ref(&self) -> ActorRef<CD::Message> {
        self.ctx().actor_ref()
    }
}

impl<CD> ActorRefFactory for Component<CD>
where
    CD: ComponentDefinition + ActorRaw + 'static,
{
    type Message = CD::Message;

    fn actor_ref(&self) -> ActorRef<CD::Message> {
        self.on_definition(|c| c.ctx().actor_ref())
    }
}

impl<CD> Dispatching for CD
where
    CD: ComponentDefinition + 'static,
{
    fn dispatcher_ref(&self) -> DispatcherRef {
        self.ctx().dispatcher_ref()
    }
}

impl<CD> ActorSource for CD
where
    CD: ComponentDefinition + 'static,
{
    fn path_resolvable(&self) -> PathResolvable {
        PathResolvable::ActorId(self.ctx().id().clone())
    }
}

impl<CD> ActorPathFactory for CD
where
    CD: ComponentDefinition + 'static,
{
    fn actor_path(&self) -> ActorPath {
        self.ctx().actor_path()
    }
}

impl<CD> Timer<CD> for CD
where
    CD: ComponentDefinition + 'static,
{
    fn schedule_once<F>(&mut self, timeout: Duration, action: F) -> ScheduledTimer
    where
        F: FnOnce(&mut CD, ScheduledTimer) + Send + 'static,
    {
        let ctx = self.ctx_mut();
        let component = ctx.component();
        ctx.timer_manager_mut()
            .schedule_once(Arc::downgrade(&component), timeout, action)
    }

    fn schedule_periodic<F>(
        &mut self,
        delay: Duration,
        period: Duration,
        action: F,
    ) -> ScheduledTimer
    where
        F: Fn(&mut CD, ScheduledTimer) + Send + 'static,
    {
        let ctx = self.ctx_mut();
        let component = ctx.component();
        ctx.timer_manager_mut()
            .schedule_periodic(Arc::downgrade(&component), delay, period, action)
    }

    fn cancel_timer(&mut self, handle: ScheduledTimer) {
        let ctx = self.ctx_mut();
        ctx.timer_manager_mut().cancel_timer(handle);
    }
}

impl<C: ComponentDefinition + Sized> CoreContainer for Component<C> {
    fn id(&self) -> &Uuid {
        &self.core.id
    }

    fn core(&self) -> &ComponentCore {
        &self.core
    }

    fn execute(&self) -> () {
        if (lifecycle::is_destroyed(&self.state)) {
            return; // don't execute anything
        }
        if (lifecycle::is_faulty(&self.state)) {
            warn!(
                self.logger,
                "Ignoring attempt to execute a faulty component!"
            );
            return; // don't execute anything
        }
        let res = panic::catch_unwind(panic::AssertUnwindSafe(|| {
            self.inner_execute();
        }));
        match res {
            Ok(_) => (), // great
            Err(e) => {
                if let Some(error_msg) = e.downcast_ref::<&str>() {
                    error!(self.logger, "Component panicked with: {:?}", error_msg);
                } else if let Some(error_msg) = e.downcast_ref::<String>() {
                    error!(self.logger, "Component panicked with: {:?}", error_msg);
                } else {
                    error!(
                        self.logger,
                        "Component panicked with a non-string message with type id={:?}",
                        e.type_id()
                    );
                }
                lifecycle::set_faulty(&self.state);
                if let Some(ref supervisor) = self.supervisor {
                    supervisor.enqueue(SupervisorMsg::Faulty(self.core.id));
                } else {
                    // we are the supervisor!
                    error!(
                        self.logger,
                        "Top level component panicked! Poisoning system."
                    );
                    self.system().poison();
                }
            }
        }
    }

    fn control_port(&self) -> ProvidedRef<ControlPort> {
        let cq = Arc::downgrade(&self.ctrl_queue);
        let cc = Arc::downgrade(&self.core.component());
        ProvidedRef::new(cc, cq)
    }

    fn schedule(&self) -> () {
        match self.custom_scheduler {
            Some(ref scheduler) => scheduler.schedule_custom(),
            None => {
                let core = self.core();
                core.system().schedule(core.component())
            }
        }
    }

    fn type_name(&self) -> &'static str {
        C::type_name()
    }
}

impl<C: ComponentDefinition + Sized> fmt::Debug for Component<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Component(id={}, ctype={})", self.id(), self.type_name())
    }
}

/// Statistics about the last invocation of [execute](ComponentDefinition::execute).
pub struct ExecuteResult {
    count: usize,
    skip: usize,
}

impl ExecuteResult {
    /// Create a new execute result
    ///
    /// `count` gives the total number of events handled during the invocation
    /// `skip` gives the offset from where queues will be checked during the next invocation (used for fairness)
    pub fn new(count: usize, skip: usize) -> ExecuteResult {
        ExecuteResult { count, skip }
    }
}

/// The contextual object for a Kompact component
///
/// Gives access compact internal features like
/// timers, logging, confguration, an the self reference.
pub struct ComponentContext<CD: ComponentDefinition + Sized + 'static> {
    inner: Option<ComponentContextInner<CD>>,
}

struct ComponentContextInner<CD: ComponentDefinition + ActorRaw + Sized + 'static> {
    timer_manager: TimerManager<CD>,
    component: Weak<Component<CD>>,
    logger: KompactLogger,
    actor_ref: ActorRef<CD::Message>,
    buffer: RefCell<Option<EncodeBuffer>>,
    config: Arc<Hocon>,
    id: Uuid,
}

impl<CD: ComponentDefinition + Sized + 'static> ComponentContext<CD> {
    /// Create a new, uninitialised component context
    pub fn new() -> ComponentContext<CD> {
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
        let id = c.id().clone();
        let inner = ComponentContextInner {
            timer_manager: TimerManager::new(system.timer_ref()),
            component: Arc::downgrade(&c),
            logger: c.logger().new(o!("ctype" => CD::type_name())),
            actor_ref: c.actor_ref(),
            buffer: RefCell::new(None),
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
    ///             ctx: ComponentContext::new(),
    ///         }
    ///     }    
    /// }
    /// impl Provide<ControlPort> for HelloLogging {
    ///     fn handle(&mut self, event: ControlEvent) -> () {
    ///         info!(self.ctx().log(), "Hello control event: {:?}", event);
    ///         if event == ControlEvent::Start {
    ///             self.ctx().system().shutdown_async();
    ///         }
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
    ///             ctx: ComponentContext::new(),
    ///         }
    ///     }    
    /// }
    /// impl Provide<ControlPort> for ConfigComponent {
    ///     fn handle(&mut self, event: ControlEvent) -> () {
    ///         match event {
    ///             ControlEvent::Start => {
    ///                 assert_eq!(Some(7i64), self.ctx().config()["a"].as_i64());
    ///                 self.ctx().system().shutdown_async();
    ///             }
    ///             _ => (), // ignore
    ///         }
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

    /// Returns the component instance wrapping this component definition
    ///
    /// This is mostly meant to be passed along for scheduling and the like.
    /// Don't try to lock anything on the already thread executing the component!
    pub fn component(&self) -> Arc<dyn CoreContainer> {
        match self.inner_ref().component.upgrade() {
            Some(ac) => ac,
            None => panic!("Component already deallocated!"),
        }
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
        let id = self.id().clone();
        ActorPath::Unique(UniquePath::with_system(self.system().system_path(), id))
    }
}

struct ContextSystemHandle {
    component: Arc<dyn CoreContainer>,
}

impl ContextSystemHandle {
    fn from(component: Arc<dyn CoreContainer>) -> Self {
        ContextSystemHandle { component }
    }

    fn send_registration(&self, env: RegistrationEnvelope) -> () {
        let envelope = MsgEnvelope::Typed(DispatchEnvelope::Registration(env));
        self.component.system().dispatcher_ref().enqueue(envelope);
    }
}

impl SystemHandle for ContextSystemHandle {
    fn create<C, F>(&self, f: F) -> Arc<Component<C>>
    where
        F: FnOnce() -> C,
        C: ComponentDefinition + 'static,
    {
        self.component.system().create(f)
    }

    #[cfg(all(nightly, feature = "type_erasure"))]
    fn create_erased<M: MessageBounds>(
        &self,
        a: Box<dyn CreateErased<M>>,
    ) -> Arc<dyn AbstractComponent<Message = M>> {
        self.component.system().create_erased(a)
    }

    fn register(
        &self,
        c: &Arc<impl AbstractComponent + ?Sized>,
        reply_to: &dyn Receiver<RegistrationResponse>,
    ) -> RegistrationId {
        let id = RegistrationId(Uuid::new_v4());
        let recipient = reply_to.recipient();
        let env = RegistrationEnvelope::with_recipient(
            c.as_ref(),
            PathResolvable::ActorId(*c.id()),
            false,
            id,
            recipient,
        );
        self.send_registration(env);
        id
    }

    fn register_without_response(&self, c: &Arc<impl AbstractComponent + ?Sized>) -> () {
        let env = RegistrationEnvelope::basic(c.as_ref(), PathResolvable::ActorId(*c.id()), false);
        self.send_registration(env);
    }

    fn register_by_alias<A>(
        &self,
        c: &Arc<impl AbstractComponent + ?Sized>,
        alias: A,
        reply_to: &dyn Receiver<RegistrationResponse>,
    ) -> RegistrationId
    where
        A: Into<String>,
    {
        let id = RegistrationId(Uuid::new_v4());
        let recipient = reply_to.recipient();
        let env = RegistrationEnvelope::with_recipient(
            c.as_ref(),
            PathResolvable::Alias(alias.into()),
            false,
            id,
            recipient,
        );
        self.send_registration(env);
        id
    }

    fn register_by_alias_without_response<A>(
        &self,
        c: &Arc<impl AbstractComponent + ?Sized>,
        alias: A,
    ) -> ()
    where
        A: Into<String>,
    {
        let env =
            RegistrationEnvelope::basic(c.as_ref(), PathResolvable::Alias(alias.into()), false);
        self.send_registration(env);
    }

    fn update_alias_registration<A>(
        &self,
        c: &Arc<impl AbstractComponent + ?Sized>,
        alias: A,
        reply_to: &dyn Receiver<RegistrationResponse>,
    ) -> RegistrationId
    where
        A: Into<String>,
    {
        let id = RegistrationId(Uuid::new_v4());
        let recipient = reply_to.recipient();
        let env = RegistrationEnvelope::with_recipient(
            c.as_ref(),
            PathResolvable::Alias(alias.into()),
            true,
            id,
            recipient,
        );
        self.send_registration(env);
        id
    }

    fn update_alias_registration_without_response<A>(
        &self,
        c: &Arc<impl AbstractComponent + ?Sized>,
        alias: A,
    ) -> ()
    where
        A: Into<String>,
    {
        let env =
            RegistrationEnvelope::basic(c.as_ref(), PathResolvable::Alias(alias.into()), true);
        self.send_registration(env);
    }

    fn start(&self, c: &Arc<impl AbstractComponent + ?Sized>) -> () {
        self.component.system().start(c)
    }

    fn stop(&self, c: &Arc<impl AbstractComponent + ?Sized>) -> () {
        self.component.system().stop(c)
    }

    fn kill(&self, c: Arc<impl AbstractComponent + ?Sized>) -> () {
        self.component.system().kill(c)
    }

    fn throughput(&self) -> usize {
        self.component.system().throughput()
    }

    fn max_messages(&self) -> usize {
        self.component.system().max_messages()
    }

    fn shutdown_async(&self) -> () {
        self.component.system().shutdown_async()
    }

    fn system_path(&self) -> SystemPath {
        self.component.system().system_path()
    }

    fn deadletter_ref(&self) -> ActorRef<Never> {
        self.component.system().actor_ref()
    }
}

impl Dispatching for ContextSystemHandle {
    fn dispatcher_ref(&self) -> DispatcherRef {
        self.component.system().dispatcher_ref()
    }
}

/// Object-safe part of [`ComponentDefinition`].
///
/// This trait aggregates all the object-safe super-traits of [`ComponentDefinition`] to make
/// trait objects possible while rust doesn't have multi-trait trait-objects.
pub trait DynamicComponentDefinition:
    DynamicPortAccess + Provide<ControlPort> + ActorRaw + Send
{
}

impl<T> DynamicComponentDefinition for T where
    T: DynamicPortAccess + Provide<ControlPort> + ActorRaw + Send
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
    Self: Sized,
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
pub trait AbstractComponent: ActorRefFactory + CoreContainer + Any {
    #[doc(hidden)] // internal api
    fn enqueue_control(&self, event: ControlEvent);

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
}

impl<C> AbstractComponent for Component<C>
where
    C: ComponentDefinition + 'static,
{
    #[doc(hidden)] // internal api
    fn enqueue_control(&self, event: ControlEvent) {
        Component::enqueue_control(self, event)
    }

    fn dyn_definition_mut(
        &mut self,
    ) -> &mut dyn DynamicComponentDefinition<Message = Self::Message> {
        self.definition_mut()
    }

    fn lock_dyn_definition(
        &self,
    ) -> Result<DynamicComponentDefinitionMutexGuard<Self::Message>, LockPoisoned> {
        let lock = self.definition.lock().map_err(|_| LockPoisoned)?;
        let res = OwningRefMut::new(Box::new(lock))
            .map_mut(|l| {
                l.deref_mut() as &mut dyn DynamicComponentDefinition<Message = Self::Message>
            })
            .erase_owner();
        Ok(res)
    }

    fn as_any(&self) -> &dyn Any {
        self
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

/// An abstraction over providers of Kompact loggers
pub trait ComponentLogging {
    /// Returns a reference to the component's logger instance
    ///
    /// See [log](ComponentContext::log) for more details.
    fn log(&self) -> &KompactLogger;
}

impl<CD> ComponentLogging for CD
where
    CD: ComponentDefinition + 'static,
{
    fn log(&self) -> &KompactLogger {
        self.ctx().log()
    }
}

/// A trait implementing handling of provided events of `P`
///
/// This is equivalent to a Kompics *Handler* subscribed on a provided port of type `P`.
pub trait Provide<P: Port + 'static> {
    /// Handle the port's `event`
    ///
    /// # Note
    ///
    /// Remember that components usually run on a shared thread pool,
    /// so you shouldn't ever block in this method unless you know what you are doing.
    fn handle(&mut self, event: P::Request) -> ();
}

/// A trait implementing handling of required events of `P`
///
/// This is equivalent to a Kompics *Handler* subscribed on a required port of type `P`.
pub trait Require<P: Port + 'static> {
    /// Handle the port's `event`
    ///
    /// # Note
    ///
    /// Remember that components usually run on a shared thread pool,
    /// so you shouldn't ever block in this method unless you know what you are doing.
    fn handle(&mut self, event: P::Indication) -> ();
}

/// A convenience abstraction over concrete port instance fields
///
/// This trait is usually automatically derived when using `#[derive(ComponentDefinition)]`.
pub trait ProvideRef<P: Port + 'static> {
    /// Returns a provided reference to this component's port instance of type `P`
    fn provided_ref(&mut self) -> ProvidedRef<P>;
    /// Connects this component's provided port instance of type `P` to `req`
    fn connect_to_required(&mut self, req: RequiredRef<P>) -> ();
}

/// A convenience abstraction over concrete port instance fields
///
/// This trait is usually automatically derived when using `#[derive(ComponentDefinition)]`.
pub trait RequireRef<P: Port + 'static> {
    /// Returns a required reference to this component's port instance of type `P`
    fn required_ref(&mut self) -> RequiredRef<P>;
    /// Connects this component's required port instance of type `P` to `prov`
    fn connect_to_provided(&mut self, prov: ProvidedRef<P>) -> ();
}

/// Same as [ProvideRef](ProvideRef), but for instances that must be locked first
///
/// This is used, for example, with an `Arc<Component<_>>`.
pub trait LockingProvideRef<P: Port + 'static> {
    /// Returns a required reference to this component's port instance of type `P`
    fn provided_ref(&self) -> ProvidedRef<P>;
    /// Connects this component's required port instance of type `P` to `prov`
    fn connect_to_required(&self, req: RequiredRef<P>) -> ();
}

/// Same as [RequireRef](RequireRef), but for instances that must be locked first
///
/// This is used, for example, with an `Arc<Component<_>>`.
pub trait LockingRequireRef<P: Port + 'static> {
    /// Returns a required reference to this component's port instance of type `P`
    fn required_ref(&self) -> RequiredRef<P>;
    /// Connects this component's required port instance of type `P` to `prov`
    fn connect_to_provided(&self, prov: ProvidedRef<P>) -> ();
}

impl<P, CD> LockingProvideRef<P> for Arc<Component<CD>>
where
    P: Port + 'static,
    CD: ComponentDefinition + Provide<P> + ProvideRef<P>,
{
    fn provided_ref(&self) -> ProvidedRef<P> {
        self.on_definition(|cd| ProvideRef::provided_ref(cd))
    }

    fn connect_to_required(&self, req: RequiredRef<P>) -> () {
        self.on_definition(|cd| ProvideRef::connect_to_required(cd, req))
    }
}

impl<P, CD> LockingRequireRef<P> for Arc<Component<CD>>
where
    P: Port + 'static,
    CD: ComponentDefinition + Require<P> + RequireRef<P>,
{
    fn required_ref(&self) -> RequiredRef<P> {
        self.on_definition(|cd| RequireRef::required_ref(cd))
    }

    fn connect_to_provided(&self, prov: ProvidedRef<P>) -> () {
        self.on_definition(|cd| RequireRef::connect_to_provided(cd, prov))
    }
}

/// Indicates whether or not a component should be sent to the [Scheduler](runtime::Scheduler)
pub enum SchedulingDecision {
    /// Sent the component to the [Scheduler](runtime::Scheduler)
    ///
    /// That is, call [schedule](CoreContainer::schedule).
    Schedule,
    /// Don't schedule the component, because it is already scheduled
    AlreadyScheduled,
    /// Don't schedule the component, because it has nothing to do
    NoWork,
}

/// The core of a Kompact component
///
/// Contains the unique id, as well as references to the Kompact system,
/// internal state variables, and the component instance itself.
pub struct ComponentCore {
    id: Uuid,
    system: KompactSystem,
    work_count: AtomicUsize,
    component: UnsafeCell<Weak<dyn CoreContainer>>,
}

impl ComponentCore {
    pub(crate) fn with<CC: CoreContainer + Sized + 'static>(
        system: KompactSystem,
    ) -> ComponentCore {
        let weak_sized = Weak::<CC>::new();
        let weak = weak_sized as Weak<dyn CoreContainer>;
        ComponentCore {
            id: Uuid::new_v4(),
            system,
            work_count: AtomicUsize::new(0),
            component: UnsafeCell::new(weak),
        }
    }

    /// Returns a reference to the Kompact system this component is a part of
    pub fn system(&self) -> &KompactSystem {
        &self.system
    }

    /// Returns the component's unique id
    pub fn id(&self) -> &Uuid {
        &self.id
    }

    pub(crate) unsafe fn set_component(&self, c: Arc<dyn CoreContainer>) -> () {
        let component_mut = self.component.get();
        *component_mut = Arc::downgrade(&c);
    }

    /// Returns the component instance itself, wrapped in an [Arc](std::sync::Arc)
    ///
    /// This method will panic if the component hasn't been properly initialised, yet!
    pub fn component(&self) -> Arc<dyn CoreContainer> {
        unsafe {
            match (*self.component.get()).upgrade() {
                Some(ac) => ac,
                None => panic!("Component already deallocated (or not initialised)!"),
            }
        }
    }

    pub(crate) fn increment_work(&self) -> SchedulingDecision {
        if self.work_count.fetch_add(1, Ordering::SeqCst) == 0 {
            SchedulingDecision::Schedule
        } else {
            SchedulingDecision::AlreadyScheduled
        }
    }

    pub(crate) fn decrement_work(&self, work_done: usize) -> SchedulingDecision {
        let oldv = self.work_count.fetch_sub(work_done, Ordering::SeqCst);
        let newv = oldv - work_done;
        if (newv > 0) {
            SchedulingDecision::Schedule
        } else {
            SchedulingDecision::NoWork
        }
    }
}

// The compiler gets stuck into a recursive loop trying to figure this out itself
//unsafe impl<C: ComponentDefinition + Sized> Send for Component<C> {}
//unsafe impl<C: ComponentDefinition + Sized> Sync for Component<C> {}

unsafe impl Send for ComponentCore {}

unsafe impl Sync for ComponentCore {}

#[cfg(test)]
mod tests {
    use crate::{component::AbstractComponent, prelude::*};
    use std::{sync::Arc, thread, time::Duration};

    #[derive(ComponentDefinition, Actor)]
    struct TestComponent {
        ctx: ComponentContext<TestComponent>,
    }

    impl TestComponent {
        fn new() -> TestComponent {
            TestComponent {
                ctx: ComponentContext::new(),
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

    #[test]
    fn component_core_send() -> () {
        let system = KompactConfig::default().build().expect("KompactSystem");
        let cc = system.create(TestComponent::new);
        let core = cc.core();
        is_send(&core.id);
        is_send(&core.system);
        is_send(&core.work_count);
        // component is clearly not Send, but that's ok
        is_sync(&core.id);
        is_sync(&core.system);
        is_sync(&core.work_count);
        // component is clearly not Sync, but that's ok
    }

    // Just a way to force the compiler to infer Send for T
    fn is_send<T: Send>(_v: &T) -> () {
        // ignore
    }

    // Just a way to force the compiler to infer Sync for T
    fn is_sync<T: Sync>(_v: &T) -> () {
        // ignore
    }

    #[derive(Debug, Copy, Clone)]
    struct TestMessage;
    impl Serialisable for TestMessage {
        fn ser_id(&self) -> SerId {
            Self::SER_ID
        }

        fn size_hint(&self) -> Option<usize> {
            Some(0)
        }

        fn serialise(&self, _buf: &mut dyn BufMut) -> Result<(), SerError> {
            Ok(())
        }

        fn local(self: Box<Self>) -> Result<Box<dyn Any + Send>, Box<dyn Serialisable>> {
            Ok(self)
        }
    }
    impl Deserialiser<TestMessage> for TestMessage {
        // whatever
        const SER_ID: SerId = 42;

        fn deserialise(_buf: &mut dyn Buf) -> Result<TestMessage, SerError> {
            Ok(TestMessage)
        }
    }

    #[derive(ComponentDefinition)]
    struct ChildComponent {
        ctx: ComponentContext<Self>,
        got_message: bool,
    }
    impl ChildComponent {
        fn new() -> Self {
            ChildComponent {
                ctx: ComponentContext::new(),
                got_message: false,
            }
        }
    }
    ignore_control!(ChildComponent);
    impl NetworkActor for ChildComponent {
        type Deserialiser = TestMessage;
        type Message = TestMessage;

        fn receive(&mut self, _sender: Option<ActorPath>, _msg: Self::Message) -> () {
            info!(self.log(), "Child got message");
            self.got_message = true;
        }
    }

    #[derive(Debug)]
    enum ParentMessage {
        GetChild(KPromise<Arc<Component<ChildComponent>>>),
        RegResp(RegistrationResponse),
    }

    impl From<RegistrationResponse> for ParentMessage {
        fn from(rr: RegistrationResponse) -> Self {
            ParentMessage::RegResp(rr)
        }
    }

    #[derive(ComponentDefinition)]
    struct ParentComponent {
        ctx: ComponentContext<Self>,
        alias_opt: Option<String>,
        child: Option<Arc<Component<ChildComponent>>>,
        reg_id: Option<RegistrationId>,
    }
    impl ParentComponent {
        fn unique() -> Self {
            ParentComponent {
                ctx: ComponentContext::new(),
                alias_opt: None,
                child: None,
                reg_id: None,
            }
        }

        fn alias(s: String) -> Self {
            ParentComponent {
                ctx: ComponentContext::new(),
                alias_opt: Some(s),
                child: None,
                reg_id: None,
            }
        }
    }
    impl Provide<ControlPort> for ParentComponent {
        fn handle(&mut self, event: ControlEvent) -> () {
            match event {
                ControlEvent::Start => {
                    let child = self.ctx.system().create(ChildComponent::new);
                    let id = match self.alias_opt.take() {
                        Some(s) => self.ctx.system().register_by_alias(&child, s, self),
                        None => self.ctx.system().register(&child, self),
                    };
                    self.reg_id = Some(id);
                    self.child = Some(child);
                }
                ControlEvent::Stop | ControlEvent::Kill => {
                    let _ = self.child.take(); // don't hang on to the child
                }
            }
        }
    }
    impl Actor for ParentComponent {
        type Message = ParentMessage;

        fn receive_local(&mut self, msg: Self::Message) -> () {
            match msg {
                ParentMessage::GetChild(promise) => {
                    if let Some(ref child) = self.child {
                        promise.fulfil(child.clone()).expect("fulfilled");
                    } else {
                        drop(promise); // this will cause an error on the Future side
                    }
                }
                ParentMessage::RegResp(res) => {
                    assert_eq!(res.id, self.reg_id.take().unwrap());
                    info!(self.log(), "Child was registered");
                    if let Some(ref child) = self.child {
                        self.ctx.system().start(child);
                        let path = res.result.expect("actor path");
                        path.tell(TestMessage, self);
                    } else {
                        unreachable!();
                    }
                }
            }
        }

        fn receive_network(&mut self, _msg: NetMessage) -> () {
            unimplemented!("Shouldn't be used");
        }
    }

    #[test]
    fn child_unique_registration_test() -> () {
        let wait_time = Duration::from_millis(1000);

        let mut conf = KompactConfig::default();
        conf.system_components(DeadletterBox::new, NetworkConfig::default().build());
        let system = conf.build().expect("system");
        let parent = system.create(ParentComponent::unique);
        system.start(&parent);
        thread::sleep(wait_time);
        let (p, f) = kpromise::<Arc<Component<ChildComponent>>>();
        parent.actor_ref().tell(ParentMessage::GetChild(p));
        let child = f.wait_timeout(wait_time).expect("child");
        let stop_f = system.stop_notify(&child);
        system.stop(&parent);

        stop_f.wait_timeout(wait_time).expect("child didn't stop");
        child.on_definition(|cd| {
            assert!(cd.got_message, "child didn't get the message");
        });
        system.shutdown().expect("shutdown");
    }

    const TEST_ALIAS: &str = "test";

    #[test]
    fn child_alias_registration_test() -> () {
        let wait_time = Duration::from_millis(1000);

        let mut conf = KompactConfig::default();
        conf.system_components(DeadletterBox::new, NetworkConfig::default().build());
        let system = conf.build().expect("system");
        let parent = system.create(|| ParentComponent::alias(TEST_ALIAS.into()));
        system.start(&parent);
        thread::sleep(wait_time);
        let (p, f) = kpromise::<Arc<Component<ChildComponent>>>();
        parent.actor_ref().tell(ParentMessage::GetChild(p));
        let child = f.wait_timeout(wait_time).expect("child");
        let stop_f = system.stop_notify(&child);
        system.stop(&parent);

        stop_f.wait_timeout(wait_time).expect("child didn't stop");
        child.on_definition(|cd| {
            assert!(cd.got_message, "child didn't get the message");
        });
        system.shutdown().expect("shutdown");
    }

    #[test]
    fn test_dynamic_port_access() -> () {
        struct A;
        impl Port for A {
            type Indication = u64;
            type Request = String;
        }
        struct B;
        impl Port for B {
            type Indication = &'static str;
            type Request = i8;
        }

        #[derive(ComponentDefinition, Actor)]
        struct TestComp {
            ctx: ComponentContext<Self>,
            req_a: RequiredPort<A>,
            prov_b: ProvidedPort<B>,
        }

        impl TestComp {
            fn new() -> TestComp {
                TestComp {
                    ctx: ComponentContext::new(),
                    req_a: RequiredPort::new(),
                    prov_b: ProvidedPort::new(),
                }
            }
        }
        ignore_control!(TestComp);
        ignore_requests!(B, TestComp);
        ignore_indications!(A, TestComp);

        let system = KompactConfig::default().build().expect("System");
        let comp = system.create(|| TestComp::new());
        let dynamic: Arc<dyn AbstractComponent<Message = Never>> = comp;
        dynamic.on_dyn_definition(|def| {
            assert!(def.get_required_port::<A>().is_some());
            assert!(def.get_provided_port::<A>().is_none());

            assert!(def.get_required_port::<B>().is_none());
            assert!(def.get_provided_port::<B>().is_some());
        });

        system.shutdown().expect("shutdown");
    }
}

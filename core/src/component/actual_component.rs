use super::*;

// just define these expansions, so I don't have to write it multiple times
macro_rules! check_and_handle_blocking {
    ($self:ident, $guard:ident,$count:ident,$res:ident) => {
        match $res {
            Handled::Ok => (),
            Handled::BlockOn(blocking_future) => {
                $guard.definition.ctx_mut().set_blocking(blocking_future);
                run_blocking!($self, $guard, $count);
            }
        }
    };
}
macro_rules! run_blocking {
    ($self:ident, $guard:ident,$count:ident) => {
        trace!($self.logger, "Component has been blocked");
        let original = $self.core.decrement_work($count);
        return $guard
            .definition
            .ctx_mut()
            .run_blocking_task()
            .or_use(|| original);
    };
}

/// A concrete component instance
///
/// The component class itself is application agnostic,
/// but it contains the application specific [ComponentDefinition](ComponentDefinition).
pub struct Component<C: ComponentDefinition + ActorRaw + Sized + 'static> {
    core: ComponentCore,
    custom_scheduler: Option<dedicated_scheduler::DedicatedThreadScheduler>,
    pub(crate) mutable_core: Mutex<ComponentMutableCore<C>>,
    ctrl_queue: Arc<ConcurrentQueue<<ControlPort as Port>::Request>>,
    msg_queue: TypedMsgQueue<C::Message>,
    // system components don't have supervision
    supervisor: Option<ProvidedRef<SupervisionPort>>,
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
        let mutable_core = ComponentMutableCore::from(definition);
        Component {
            core,
            custom_scheduler: None,
            mutable_core: Mutex::new(mutable_core),
            ctrl_queue: Arc::new(ConcurrentQueue::new()),
            msg_queue: TypedMsgQueue::new(),
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
        let mutable_core = ComponentMutableCore::from(definition);
        Component {
            core,
            custom_scheduler: Some(custom_scheduler),
            mutable_core: Mutex::new(mutable_core),
            ctrl_queue: Arc::new(ConcurrentQueue::new()),
            msg_queue: TypedMsgQueue::new(),
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
        let mutable_core = ComponentMutableCore::from(definition);
        Component {
            core,
            custom_scheduler: None,
            mutable_core: Mutex::new(mutable_core),
            ctrl_queue: Arc::new(ConcurrentQueue::new()),
            msg_queue: TypedMsgQueue::new(),
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
        let mutable_core = ComponentMutableCore::from(definition);
        Component {
            core,
            custom_scheduler: Some(custom_scheduler),
            mutable_core: Mutex::new(mutable_core),
            ctrl_queue: Arc::new(ConcurrentQueue::new()),
            msg_queue: TypedMsgQueue::new(),
            supervisor: None,
            logger,
        }
    }

    // pub(crate) fn msg_queue_wref(&self) -> Weak<ConcurrentQueue<MsgEnvelope>> {
    //     Arc::downgrade(&self.msg_queue)
    // }

    pub(crate) fn enqueue_control(&self, event: <ControlPort as Port>::Request) -> () {
        self.ctrl_queue.push(event);
        if let SchedulingDecision::Schedule = self.core.increment_work() {
            self.schedule();
        }
    }

    /// Returns a mutable reference to the underlying component definition.
    ///
    /// This can only be done if you have a reference to the component instance
    /// that isn't hidden behind an [Arc](std::sync::Arc). For example,
    /// after the system shuts down and your code holds on to the last reference to a component
    /// you can use [get_mut](std::sync::Arc::get_mut) or [try_unwrap](std::sync::Arc::try_unwrap).
    pub fn definition_mut(&mut self) -> &mut C {
        self.mutable_core
            .get_mut()
            .map(|core| &mut core.definition)
            .unwrap()
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
        let mut cd = self.mutable_core.lock().unwrap();
        f(&mut cd.definition)
    }

    /// Returns a reference to this component's logger
    pub fn logger(&self) -> &KompactLogger {
        &self.logger
    }

    /// Returns `true` if the component is marked as *faulty*.
    pub fn is_faulty(&self) -> bool {
        lifecycle::is_faulty(&self.core.state)
    }

    /// Returns `true` if the component is marked as *active*.
    pub fn is_active(&self) -> bool {
        lifecycle::is_active(&self.core.state)
    }

    /// Returns `true` if the component is marked as *destroyed*.
    pub fn is_destroyed(&self) -> bool {
        lifecycle::is_destroyed(&self.core.state)
    }

    pub(super) fn set_blocking(&self) {
        lifecycle::set_blocking(&self.core.state)
    }

    pub(super) fn set_active(&self) {
        lifecycle::set_active(&self.core.state)
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

    fn inner_execute(&self) -> SchedulingDecision {
        let max_events = self.core.system.throughput();
        let max_messages = self.core.system.max_messages();

        match self.mutable_core.lock() {
            Ok(mut guard) => {
                if guard.definition.ctx().is_blocking() {
                    return guard
                        .definition
                        .ctx_mut()
                        .run_blocking_task()
                        .or_use(|| self.core.get_scheduling_decision());
                }

                let mut count: usize = 0;
                while let Ok(event) = self.ctrl_queue.pop() {
                    // ignore max_events for lifecyle events
                    // println!("Executing event: {:?}", event);
                    let res = match event {
                        lifecycle::ControlEvent::Start => {
                            lifecycle::set_active(&self.core.state);
                            debug!(self.logger, "Component started.");
                            let supervisor_msg = SupervisorMsg::Started(self.core.component());
                            let res = guard.definition.handle(event);
                            count += 1;
                            // inform supervisor after local handling to make sure crashing component don't count as started
                            if let Some(ref supervisor) = self.supervisor {
                                supervisor.enqueue(supervisor_msg);
                            }
                            res
                        }
                        lifecycle::ControlEvent::Stop => {
                            lifecycle::set_passive(&self.core.state);
                            debug!(self.logger, "Component stopped.");
                            let supervisor_msg = SupervisorMsg::Stopped(self.core.id);
                            let res = guard.definition.handle(event);
                            count += 1;
                            // inform supervisor after local handling to make sure crashing component don't count as started
                            if let Some(ref supervisor) = self.supervisor {
                                supervisor.enqueue(supervisor_msg);
                            }
                            res
                        }
                        lifecycle::ControlEvent::Kill => {
                            lifecycle::set_destroyed(&self.core.state);
                            debug!(self.logger, "Component killed.");
                            let supervisor_msg = SupervisorMsg::Killed(self.core.id);
                            let res = guard.definition.handle(event);
                            count += 1;
                            // inform supervisor after local handling to make sure crashing component don't count as started
                            if let Some(ref supervisor) = self.supervisor {
                                supervisor.enqueue(supervisor_msg);
                            }
                            res
                        }
                        lifecycle::ControlEvent::Poll(tag) => {
                            let res = guard.definition.ctx_mut().run_nonblocking_task(tag);
                            count += 1;
                            res
                        }
                    };

                    match res {
                        Handled::Ok => (), // fine continue
                        Handled::BlockOn(_blocking_future) => {
                            // There are some tricky edge cases here about delaying shutdown when blocking on futures, etc
                            unimplemented!("Blocking in lifecycle events is not yet supported!");
                            //guard.definition.ctx_mut().set_blocking(blocking_future);
                        }
                    }
                }
                if (!lifecycle::is_active(&self.core.state)) {
                    trace!(self.logger, "Not running inactive scheduled.");
                    return self.core.decrement_work(count);
                }
                // timers have highest priority
                while count < max_events {
                    let c = &mut guard.definition;
                    let res: Handled = match c.ctx_mut().timer_manager_mut().try_action() {
                        ExecuteAction::Once(id, action) => {
                            let res = action(c, id);
                            count += 1;
                            res
                        }
                        ExecuteAction::Periodic(id, action) => {
                            let res = action(c, id);
                            count += 1;
                            res
                        }
                        ExecuteAction::None => break,
                    };
                    check_and_handle_blocking!(self, guard, count, res);
                }
                // then some messages
                while count < max_messages {
                    if let Some(env) = self.msg_queue.pop() {
                        let res = guard.definition.receive(env);
                        count += 1;
                        check_and_handle_blocking!(self, guard, count, res);
                    } else {
                        break;
                    }
                }
                // then events
                let rem_events = max_events.saturating_sub(count);
                if (rem_events > 0) {
                    let skip = guard.skip;
                    let res = guard.definition.execute(rem_events, skip);
                    guard.skip = res.skip;
                    count += res.count;
                    if res.blocking {
                        run_blocking!(self, guard, count);
                    }

                    // and maybe some more messages
                    while count < max_events {
                        if let Some(env) = self.msg_queue.pop() {
                            let res = guard.definition.receive(env);
                            count += 1;
                            check_and_handle_blocking!(self, guard, count, res);
                        } else {
                            break;
                        }
                    }
                }
                self.core.decrement_work(count)
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
        ActorRef::new(comp)
    }
}

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
        PathResolvable::ActorId(*self.ctx().id())
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
        F: FnOnce(&mut CD, ScheduledTimer) -> Handled + Send + 'static,
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
        F: Fn(&mut CD, ScheduledTimer) -> Handled + Send + 'static,
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
    fn id(&self) -> Uuid {
        self.core.id
    }

    fn core(&self) -> &ComponentCore {
        &self.core
    }

    fn execute(&self) -> SchedulingDecision {
        match self.core.load_state() {
            LifecycleState::Destroyed => return SchedulingDecision::NoWork, // don't execute anything
            LifecycleState::Faulty => {
                warn!(
                    self.logger,
                    "Ignoring attempt to execute a faulty component!"
                );
                return SchedulingDecision::NoWork; // don't execute anything
            }
            _ => (), // it's fine to continue
        }
        let res = panic::catch_unwind(panic::AssertUnwindSafe(|| self.inner_execute()));
        match res {
            Ok(decision) => decision, // great
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
                lifecycle::set_faulty(&self.core.state);
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
                SchedulingDecision::NoWork
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

    fn dyn_message_queue(&self) -> &dyn DynMsgQueue {
        &self.msg_queue
    }

    fn enqueue_control(&self, event: <ControlPort as Port>::Request) -> () {
        Component::enqueue_control(self, event)
    }
}

impl<C: ComponentDefinition + Sized> MsgQueueContainer for Component<C> {
    type Message = C::Message;

    fn message_queue(&self) -> &TypedMsgQueue<Self::Message> {
        &self.msg_queue
    }

    fn downgrade_dyn(self: Arc<Self>) -> Weak<dyn CoreContainer> {
        let c: Arc<dyn CoreContainer> = self;
        Arc::downgrade(&c)
    }
}

impl<C: ComponentDefinition + Sized> fmt::Debug for Component<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Component(id={}, ctype={})", self.id(), self.type_name())
    }
}

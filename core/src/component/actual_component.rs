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
            Handled::DieNow => {
                lifecycle::set_destroyed(&$self.core.state);
                debug!($self.logger, "Component killed via Handled.");
                let supervisor_msg = SupervisorMsg::Killed($self.core.id);
                let _res = $guard.definition.on_kill();
                // count is irrelevant if we are anyway dying
                // $count += 1;
                // inform supervisor after local handling to make sure crashing component don't count as killed
                if let Some(ref supervisor) = $self.supervisor {
                    supervisor.enqueue(supervisor_msg);
                }
                return SchedulingDecision::NoWork
            }
        }
    };
}
macro_rules! run_blocking {
    ($self:ident, $guard:ident,$count:ident) => {
        trace!($self.logger, "Component has been blocked");
        let run_res = $guard.definition.ctx_mut().run_blocking_task();
        // make sure this read comes after running the blocking task,
        // to catch updates during the task (e.g. self messages).
        let scheduling_decision = $self.core.decrement_work($count);
        return run_res.or_use(scheduling_decision);
    };
}

pub(crate) type RecoveryFunction = dyn FnOnce(FaultContext) -> RecoveryHandler + Send + 'static;

/// Kompact's default fault recovery policy is to simply ignore the fault
pub fn default_recovery_function(ctx: FaultContext) -> RecoveryHandler {
    ctx.ignore()
}

/// A concrete component instance
///
/// The component class itself is application agnostic,
/// but it contains the application specific [ComponentDefinition](ComponentDefinition).
pub struct Component<CD: ComponentTraits> {
    core: ComponentCore,
    custom_scheduler: Option<dedicated_scheduler::DedicatedThreadScheduler>,
    pub(crate) mutable_core: Mutex<ComponentMutableCore<CD>>,
    ctrl_queue: ConcurrentQueue<ControlEvent>,
    msg_queue: TypedMsgQueue<CD::Message>,
    // system components don't have supervision
    supervisor: Option<ProvidedRef<SupervisionPort>>,
    logger: KompactLogger,
    recovery_function: Mutex<Box<RecoveryFunction>>,
}

impl<CD: ComponentTraits> Component<CD> {
    pub(crate) fn new(
        system: KompactSystem,
        definition: CD,
        supervisor: ProvidedRef<SupervisionPort>,
    ) -> Self {
        let core = ComponentCore::with::<Component<CD>>(system);
        let logger = core
            .system
            .logger()
            .new(o!("cid" => format!("{}", core.id)));
        let mutable_core = ComponentMutableCore::from(definition);
        Component {
            core,
            custom_scheduler: None,
            mutable_core: Mutex::new(mutable_core),
            ctrl_queue: ConcurrentQueue::new(),
            msg_queue: TypedMsgQueue::new(),
            supervisor: Some(supervisor),
            logger,
            recovery_function: Mutex::new(Box::new(default_recovery_function)),
        }
    }

    pub(crate) fn with_dedicated_scheduler(
        system: KompactSystem,
        definition: CD,
        supervisor: ProvidedRef<SupervisionPort>,
        custom_scheduler: dedicated_scheduler::DedicatedThreadScheduler,
    ) -> Self {
        let core = ComponentCore::with::<Component<CD>>(system);
        let logger = core
            .system
            .logger()
            .new(o!("cid" => format!("{}", core.id)));
        let mutable_core = ComponentMutableCore::from(definition);
        Component {
            core,
            custom_scheduler: Some(custom_scheduler),
            mutable_core: Mutex::new(mutable_core),
            ctrl_queue: ConcurrentQueue::new(),
            msg_queue: TypedMsgQueue::new(),
            supervisor: Some(supervisor),
            logger,
            recovery_function: Mutex::new(Box::new(default_recovery_function)),
        }
    }

    pub(crate) fn without_supervisor(system: KompactSystem, definition: CD) -> Self {
        let core = ComponentCore::with::<Component<CD>>(system);
        let logger = core
            .system
            .logger()
            .new(o!("cid" => format!("{}", core.id)));
        let mutable_core = ComponentMutableCore::from(definition);
        Component {
            core,
            custom_scheduler: None,
            mutable_core: Mutex::new(mutable_core),
            ctrl_queue: ConcurrentQueue::new(),
            msg_queue: TypedMsgQueue::new(),
            supervisor: None,
            logger,
            recovery_function: Mutex::new(Box::new(default_recovery_function)),
        }
    }

    pub(crate) fn without_supervisor_with_dedicated_scheduler(
        system: KompactSystem,
        definition: CD,
        custom_scheduler: dedicated_scheduler::DedicatedThreadScheduler,
    ) -> Self {
        let core = ComponentCore::with::<Component<CD>>(system);
        let logger = core
            .system
            .logger()
            .new(o!("cid" => format!("{}", core.id)));
        let mutable_core = ComponentMutableCore::from(definition);
        Component {
            core,
            custom_scheduler: Some(custom_scheduler),
            mutable_core: Mutex::new(mutable_core),
            ctrl_queue: ConcurrentQueue::new(),
            msg_queue: TypedMsgQueue::new(),
            supervisor: None,
            logger,
            recovery_function: Mutex::new(Box::new(default_recovery_function)),
        }
    }

    pub(crate) fn enqueue_control(&self, event: ControlEvent) -> () {
        let res = self.core.increment_work(); // must do it in this order to maintain counting guarantees
        self.ctrl_queue.push(event);
        //println!("ELLOOOO");
        if let SchedulingDecision::Schedule = res {
            self.schedule();
        }
    }

    /// Returns a mutable reference to the underlying component definition.
    ///
    /// This can only be done if you have a reference to the component instance
    /// that isn't hidden behind an [Arc](std::sync::Arc). For example,
    /// after the system shuts down and your code holds on to the last reference to a component
    /// you can use [get_mut](std::sync::Arc::get_mut) or [try_unwrap](std::sync::Arc::try_unwrap).
    pub fn definition_mut(&mut self) -> &mut CD {
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
        F: FnOnce(&mut CD) -> T,
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

    pub(super) fn set_passive(&self) {
        lifecycle::set_passive(&self.core.state);
        if let Some(ref supervisor) = self.supervisor {
            let supervisor_msg = SupervisorMsg::Stopped(self.core.id);
            supervisor.enqueue(supervisor_msg);
        }
        debug!(self.logger, "Component stopped");
    }

    pub(super) fn set_destroyed(&self) {
        lifecycle::set_destroyed(&self.core.state);
        let supervisor_msg = SupervisorMsg::Killed(self.core.id);
        if let Some(ref supervisor) = self.supervisor {
            supervisor.enqueue(supervisor_msg);
        }
        debug!(self.logger, "Component killed");
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

    /// Set the recovery function for this component
    ///
    /// See [RecoveryHandler](crate::prelude::RecoveryHandler) for more information.
    pub fn set_recovery_function<F>(&self, f: F) -> ()
    where
        F: FnOnce(FaultContext) -> RecoveryHandler + Send + 'static,
    {
        let boxed = Box::new(f);
        let mut current = self.recovery_function.lock().unwrap();
        *current = boxed;
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
                        .or_from(|| self.core.get_scheduling_decision());
                }
                //println!("before first loop inner execute");
                let mut count: usize = 0;
                while let Ok(event) = self.ctrl_queue.pop() {
                    // ignore max_events for lifecyle events
                    // println!("Executing event: {:?}", event);
                    let res = match event {
                        lifecycle::ControlEvent::Start => {
                            //println!("Start");
                            lifecycle::set_active(&self.core.state);
                            debug!(self.logger, "Component started.");
                            //println!("between set_active and on_start");
                            let res = guard.definition.on_start();
                            //println!("after on_start");
                            count += 1;
                            // inform supervisor after local handling to make sure crashing component don't count as started
                            if let Some(ref supervisor) = self.supervisor {
                                //println!("begin if on Start");
                                let supervisor_msg = SupervisorMsg::Started(self.core.component());
                                supervisor.enqueue(supervisor_msg);
                            }
                            //println!("end Start");
                            res
                        }
                        lifecycle::ControlEvent::Stop => {
                            //println!("Stop");
                            lifecycle::set_passive(&self.core.state);
                            debug!(self.logger, "Component stopping");
                            let res = guard.definition.on_stop();
                            count += 1;
                            if res.is_ok() {
                                // inform supervisor after local handling to make sure crashing component don't count as started
                                self.set_passive();
                            } // otherwise this will happen after unblock
                            res
                        }
                        lifecycle::ControlEvent::Kill => {
                            //println!("Kill");
                            lifecycle::set_destroyed(&self.core.state);
                            debug!(self.logger, "Component dying");
                            let res = guard.definition.on_kill();
                            count += 1;
                            if res.is_ok() {
                                // inform supervisor after local handling to make sure crashing component don't count as started
                                self.set_destroyed();
                            } // otherwise this will happen after unblock
                            res
                        }
                        lifecycle::ControlEvent::Poll(tag) => {
                            //println!("Poll");
                            let res = guard.definition.ctx_mut().run_nonblocking_task(tag);
                            count += 1;
                            res
                        }
                    };

                    match res {
                        Handled::Ok => (), // fine continue
                        Handled::BlockOn(blocking_future) => {
                            let transition = match event {
                                lifecycle::ControlEvent::Stop => StateTransition::Passive,
                                lifecycle::ControlEvent::Kill => StateTransition::Destroyed,
                                _ => StateTransition::Active,
                            };
                            guard
                                .definition
                                .ctx_mut()
                                .set_blocking_with_state(blocking_future, transition);
                            run_blocking!(self, guard, count);
                        }
                        Handled::DieNow => {
                            lifecycle::set_destroyed(&self.core.state);
                            debug!(self.logger, "Component killed via Handled.");
                            let supervisor_msg = SupervisorMsg::Killed(self.core.id);
                            let _res = guard.definition.on_kill();
                            // count is irrelevant when we are dying
                            if let Some(ref supervisor) = self.supervisor {
                                supervisor.enqueue(supervisor_msg);
                            }
                            return SchedulingDecision::NoWork;
                        }
                    }
                }
                //println!("after first loop inner execute");
                if !lifecycle::is_active(&self.core.state) {
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
                if rem_events > 0 {
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

impl<CD: ComponentTraits> ActorRefFactory for Arc<Component<CD>> {
    type Message = CD::Message;

    fn actor_ref(&self) -> ActorRef<Self::Message> {
        let comp = Arc::downgrade(self);
        ActorRef::new(comp)
    }
}

impl<CD: ComponentDefinition> ActorRefFactory for CD {
    type Message = CD::Message;

    fn actor_ref(&self) -> ActorRef<Self::Message> {
        self.ctx().actor_ref()
    }
}

impl<M: MessageBounds> ActorRefFactory for Arc<dyn AbstractComponent<Message = M>> {
    type Message = M;

    fn actor_ref(&self) -> ActorRef<Self::Message> {
        let comp = self.clone().as_queue_container();
        ActorRef::new(comp)
    }
}

impl<CD: ComponentTraits> DynActorRefFactory for Arc<Component<CD>> {
    fn dyn_ref(&self) -> DynActorRef {
        self.actor_ref().dyn_ref()
    }
}

impl<CD: ComponentTraits> UniqueRegistrable for Arc<Component<CD>> {
    fn component_id(&self) -> Uuid {
        self.id()
    }
}

impl<CD: ComponentTraits> Dispatching for CD {
    fn dispatcher_ref(&self) -> DispatcherRef {
        self.ctx().dispatcher_ref()
    }
}

impl<CD: ComponentTraits> ActorPathFactory for CD {
    fn actor_path(&self) -> ActorPath {
        self.ctx().actor_path()
    }
}

impl<CD: ComponentTraits> Timer<CD> for CD {
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

impl<CD: ComponentTraits> CoreContainer for Component<CD> {
    fn id(&self) -> Uuid {
        self.core.id
    }

    fn core(&self) -> &ComponentCore {
        &self.core
    }

    fn execute(&self) -> SchedulingDecision {
        //println!("execute begin");
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
        //println!("Before inner execute");
        let res = panic::catch_unwind(panic::AssertUnwindSafe(|| self.inner_execute()));
        //println!("After inner execute");
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
                    if let Ok(mut guard) = self.recovery_function.lock() {
                        let context = FaultContext::new(self.core.id, e);
                        let mut recovery_function: Box<RecoveryFunction> =
                            Box::new(default_recovery_function);
                        // replace fault handler with default, so we can own the correct one
                        // (it's anyway only going to be called once)
                        std::mem::swap(guard.deref_mut(), &mut recovery_function);
                        let handler = recovery_function(context);
                        supervisor.enqueue(SupervisorMsg::Faulty(handler));
                    } else {
                        error!(
                        self.logger,
                        "A recovery function mutex was poisoned in component of type {} with id {}. This component can not recover from its fault!",
                        CD::type_name(), self.core.id
                    );
                    }
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

    fn schedule(&self) -> () {
        match self.custom_scheduler {
            Some(ref scheduler) => scheduler.schedule_custom(),
            None => {
                let core = self.core();
                //println!("ACTUAL COMP");
                core.system().schedule(core.component())
            }
        }
    }

    fn type_name(&self) -> &'static str {
        CD::type_name()
    }

    fn dyn_message_queue(&self) -> &dyn DynMsgQueue {
        &self.msg_queue
    }

    fn enqueue_control(&self, event: ControlEvent) -> () {
        Component::enqueue_control(self, event)
    }
}

impl<CD: ComponentTraits> MsgQueueContainer for Component<CD> {
    type Message = CD::Message;

    fn message_queue(&self) -> &TypedMsgQueue<Self::Message> {
        &self.msg_queue
    }

    fn downgrade_dyn(self: Arc<Self>) -> Weak<dyn CoreContainer> {
        let c: Arc<dyn CoreContainer> = self;
        Arc::downgrade(&c)
    }
}

impl<CD: ComponentTraits> fmt::Debug for Component<CD> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Component(id={}, ctype={})", self.id(), self.type_name())
    }
}

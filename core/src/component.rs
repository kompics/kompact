use std::cell::RefCell;
use std::fmt;
use std::ops::DerefMut;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use uuid::Uuid;

use super::*;
use messaging::{DispatchEnvelope, MsgEnvelope, PathResolvable, ReceiveEnvelope};
use supervision::*;

pub trait CoreContainer: Send + Sync {
    fn id(&self) -> &Uuid;
    fn core(&self) -> &ComponentCore;
    fn execute(&self) -> ();

    fn control_port(&self) -> ProvidedRef<ControlPort>;
    //fn actor_ref(&self) -> ActorRef;
    fn system(&self) -> KompicsSystem {
        self.core().system().clone()
    }
}

impl fmt::Debug for CoreContainer {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "CoreContainer({})", self.id())
    }
}

pub struct Component<C: ComponentDefinition + Sized + 'static> {
    core: ComponentCore,
    definition: Mutex<C>,
    ctrl_queue: Arc<ConcurrentQueue<<ControlPort as Port>::Request>>,
    msg_queue: Arc<ConcurrentQueue<MsgEnvelope>>,
    skip: AtomicUsize,
    state: AtomicUsize,
    supervisor: Option<ProvidedRef<SupervisionPort>>, // system components don't have supervision
    logger: KompicsLogger,
}

impl<C: ComponentDefinition + Sized> Component<C> {
    pub(crate) fn new(
        system: KompicsSystem,
        definition: C,
        supervisor: ProvidedRef<SupervisionPort>,
    ) -> Component<C> {
        let core = ComponentCore::with(system);
        let logger = core
            .system
            .logger()
            .new(o!("cid" => format!("{}", core.id)));
        let msg_queue = Arc::new(ConcurrentQueue::new());
        Component {
            core,
            definition: Mutex::new(definition),
            ctrl_queue: Arc::new(ConcurrentQueue::new()),
            msg_queue,
            skip: AtomicUsize::new(0),
            state: lifecycle::initial_state(),
            supervisor: Some(supervisor),
            logger,
        }
    }

    pub(crate) fn without_supervisor(system: KompicsSystem, definition: C) -> Component<C> {
        let core = ComponentCore::with(system);
        let logger = core
            .system
            .logger()
            .new(o!("cid" => format!("{}", core.id)));
        Component {
            core,
            definition: Mutex::new(definition),
            ctrl_queue: Arc::new(ConcurrentQueue::new()),
            msg_queue: Arc::new(ConcurrentQueue::new()),
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
                let system = self.core.system();
                system.schedule(self.core.component());
            }
            _ => (), // nothing
        }
    }

    pub fn definition(&self) -> &Mutex<C> {
        &self.definition
    }
    pub fn definition_mut(&mut self) -> &mut C {
        self.definition.get_mut().unwrap()
    }

    pub fn on_definition<T, F>(&self, f: F) -> T
    where
        F: FnOnce(&mut C) -> T,
    {
        let mut cd = self.definition.lock().unwrap();
        f(cd.deref_mut())
    }

    pub fn logger(&self) -> &KompicsLogger {
        &self.logger
    }
}

impl<CD> ActorRefFactory for Arc<Component<CD>>
where
    CD: ComponentDefinition + 'static,
{
    fn actor_ref(&self) -> ActorRef {
        let comp = Arc::downgrade(self);
        let msgq = Arc::downgrade(&self.msg_queue);
        ActorRef::new(comp, msgq)
    }
}

impl<CD> ActorRefFactory for CD
where
    CD: ComponentDefinition + 'static,
{
    fn actor_ref(&self) -> ActorRef {
        self.ctx().actor_ref()
    }
}

impl<CD> Dispatching for CD
where
    CD: ComponentDefinition + 'static,
{
    fn dispatcher_ref(&self) -> ActorRef {
        self.ctx().dispatcher_ref()
    }
}

impl<CD> ActorSource for CD
where
    CD: ComponentDefinition + 'static,
{
    fn path_resolvable(&self) -> PathResolvable {
        PathResolvable::ActorId(self.ctx().id())
    }
}

impl<CD> Timer<CD> for CD
where
    CD: ComponentDefinition + 'static,
{
    fn schedule_once<F>(&mut self, timeout: Duration, action: F) -> ScheduledTimer
    where
        F: FnOnce(&mut CD, Uuid) + Send + 'static,
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
        F: Fn(&mut CD, Uuid) + Send + 'static,
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

pub trait ExecuteSend {
    fn execute_send(&mut self, env: DispatchEnvelope) -> () {
        panic!("Sent messages should go to the dispatcher! {:?}", env);
    }
}

impl<A: ActorRaw> ExecuteSend for A {}

impl<D: Dispatcher + ActorRaw> ExecuteSend for D {
    fn execute_send(&mut self, env: DispatchEnvelope) -> () {
        Dispatcher::receive(self, env)
    }
}

impl<C: ComponentDefinition + ExecuteSend + Sized> CoreContainer for Component<C> {
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
        let max_events = self.core.system.throughput();
        let max_messages = self.core.system.max_messages();
        match self.definition().lock() {
            Ok(mut guard) => {
                let mut count: usize = 0;
                while let Some(event) = self.ctrl_queue.try_pop() {
                    match event {
                        lifecycle::ControlEvent::Start => {
                            lifecycle::set_active(&self.state);
                            if let Some(ref supervisor) = self.supervisor {
                                supervisor.enqueue(SupervisorMsg::Started(self.core.component()));
                            }
                            debug!(self.logger, "Component started.");
                        }
                        lifecycle::ControlEvent::Stop => {
                            lifecycle::set_passive(&self.state);
                            if let Some(ref supervisor) = self.supervisor {
                                supervisor.enqueue(SupervisorMsg::Stopped(self.core.id));
                            }
                            debug!(self.logger, "Component stopped.");
                        }
                        lifecycle::ControlEvent::Kill => {
                            lifecycle::set_destroyed(&self.state);
                            if let Some(ref supervisor) = self.supervisor {
                                supervisor.enqueue(SupervisorMsg::Killed(self.core.id));
                            }
                            debug!(self.logger, "Component killed.");
                        }
                    }
                    // ignore max_events for lifecyle events
                    // println!("Executing event: {:?}", event);
                    guard.handle(event);
                    count += 1;
                }
                if (!lifecycle::is_active(&self.state)) {
                    trace!(self.logger, "Not running inactive scheduled.");
                    match self.core.decrement_work(count) {
                        SchedulingDecision::Schedule => {
                            let system = self.core.system();
                            let cc = self.core.component();
                            system.schedule(cc);
                        }
                        _ => (), // ignore
                    }
                    return;
                }
                // timers have highest priority
                while count < max_events {
                    let c = guard.deref_mut();
                    match c.ctx_mut().timer_manager_mut().try_action() {
                        ExecuteAction::Once(id, action) => {
                            action.call_box((c, id));
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
                    if let Some(env) = self.msg_queue.try_pop() {
                        match env {
                            MsgEnvelope::Receive(renv) => guard.receive(renv),
                            MsgEnvelope::Dispatch(DispatchEnvelope::Cast(cenv)) => {
                                let renv = ReceiveEnvelope::Cast(cenv);
                                guard.receive(renv);
                            }
                            MsgEnvelope::Dispatch(senv) => {
                                guard.execute_send(senv);
                            }
                        }
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
                        if let Some(env) = self.msg_queue.try_pop() {
                            match env {
                                MsgEnvelope::Receive(renv) => guard.receive(renv),
                                MsgEnvelope::Dispatch(DispatchEnvelope::Cast(cenv)) => {
                                    let renv = ReceiveEnvelope::Cast(cenv);
                                    guard.receive(renv);
                                }
                                MsgEnvelope::Dispatch(senv) => {
                                    guard.execute_send(senv);
                                }
                            }
                            count += 1;
                        } else {
                            break;
                        }
                    }
                }
                match self.core.decrement_work(count) {
                    SchedulingDecision::Schedule => {
                        let system = self.core.system();
                        let cc = self.core.component();
                        system.schedule(cc);
                    }
                    _ => (), // ignore
                }
            }
            _ => {
                panic!("System poisoned!"); //TODO better error handling
            }
        }
    }

    fn control_port(&self) -> ProvidedRef<ControlPort> {
        let cq = Arc::downgrade(&self.ctrl_queue);
        let cc = Arc::downgrade(&self.core.component());
        ProvidedRef::new(cc, cq)
    }

    // fn actor_ref(&self) -> ActorRef {
    //     let msgq = Arc::downgrade(&self.msg_queue);
    //     let cc = Arc::downgrade(&self.core.component());
    //     ActorRef::new(cc, msgq)
    // }
}

//
//pub trait Component: CoreContainer {
//    fn setup_ports(&mut self, self_component: Arc<Mutex<Self>>) -> ();
//}

pub struct ExecuteResult {
    count: usize,
    skip: usize,
}

impl ExecuteResult {
    pub fn new(count: usize, skip: usize) -> ExecuteResult {
        ExecuteResult { count, skip }
    }
}

pub struct ComponentContext<CD: ComponentDefinition + Sized + 'static> {
    inner: Option<ComponentContextInner<CD>>,
}

struct ComponentContextInner<CD: ComponentDefinition + Sized + 'static> {
    timer_manager: TimerManager<CD>,
    component: Weak<Component<CD>>,
    logger: KompicsLogger,
    actor_ref: ActorRef,
}

impl<CD: ComponentDefinition + Sized + 'static> ComponentContext<CD> {
    pub fn new() -> ComponentContext<CD> {
        ComponentContext { inner: None }
    }

    pub fn initialise(&mut self, c: Arc<Component<CD>>) -> ()
    where
        CD: ComponentDefinition + 'static,
    {
        let system = c.system();
        let inner = ComponentContextInner {
            timer_manager: TimerManager::new(system.timer_ref()),
            component: Arc::downgrade(&c),
            logger: c.logger().new(o!("ctype" => CD::type_name())),
            actor_ref: c.actor_ref(),
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

    pub fn log(&self) -> &KompicsLogger {
        &self.inner_ref().logger
    }

    pub(crate) fn timer_manager_mut(&mut self) -> &mut TimerManager<CD> {
        &mut self.inner_mut().timer_manager
    }

    pub fn component(&self) -> Arc<CoreContainer> {
        match self.inner_ref().component.upgrade() {
            Some(ac) => ac,
            None => panic!("Component already deallocated!"),
        }
    }

    pub fn system(&self) -> KompicsSystem {
        self.component().system()
    }

    pub fn dispatcher_ref(&self) -> ActorRef {
        self.system().dispatcher_ref()
    }

    pub fn id(&self) -> Uuid {
        self.component().id().clone()
    }
}

impl<CD: ComponentDefinition + Sized + 'static> ActorRefFactory for ComponentContext<CD> {
    fn actor_ref(&self) -> ActorRef {
        self.inner_ref().actor_ref.clone()
    }
}

pub trait ComponentDefinition: Provide<ControlPort> + ActorRaw + Send
where
    Self: Sized,
{
    fn setup(&mut self, self_component: Arc<Component<Self>>) -> ();
    fn execute(&mut self, max_events: usize, skip: usize) -> ExecuteResult;
    fn ctx(&self) -> &ComponentContext<Self>;
    fn ctx_mut(&mut self) -> &mut ComponentContext<Self>;
    fn type_name() -> &'static str;
}

pub trait Provide<P: Port + 'static> {
    fn handle(&mut self, event: P::Request) -> ();
}

pub trait Require<P: Port + 'static> {
    fn handle(&mut self, event: P::Indication) -> ();
}

pub enum SchedulingDecision {
    Schedule,
    AlreadyScheduled,
    NoWork,
}

pub struct ComponentCore {
    id: Uuid,
    system: KompicsSystem,
    work_count: AtomicUsize,
    component: RefCell<Option<Weak<CoreContainer>>>,
}

impl ComponentCore {
    pub fn with(system: KompicsSystem) -> ComponentCore {
        ComponentCore {
            id: Uuid::new_v4(),
            system,
            work_count: AtomicUsize::new(0),
            component: RefCell::default(),
        }
    }

    pub fn system(&self) -> &KompicsSystem {
        &self.system
    }

    pub fn id(&self) -> &Uuid {
        &self.id
    }

    pub(crate) fn set_component(&self, c: Arc<CoreContainer>) -> () {
        *self.component.borrow_mut() = Some(Arc::downgrade(&c));
    }

    pub fn component(&self) -> Arc<CoreContainer> {
        match *self.component.borrow() {
            Some(ref c) => match c.upgrade() {
                Some(ac) => ac,
                None => panic!("Component already deallocated!"),
            },
            None => panic!("Component improperly initialised!"),
        }
    }

    pub(crate) fn increment_work(&self) -> SchedulingDecision {
        if self.work_count.fetch_add(1, Ordering::SeqCst) == 0 {
            SchedulingDecision::Schedule
        } else {
            SchedulingDecision::AlreadyScheduled
        }
    }
    pub fn decrement_work(&self, work_done: usize) -> SchedulingDecision {
        //        let oldv: isize = match work_done_u.checked_as_num() {
        //            Some(work_done) => self.work_count.fetch_sub(work_done, Ordering::SeqCst),
        //            None => {
        //
        //            }
        //        }
        let oldv = self.work_count.fetch_sub(work_done, Ordering::SeqCst);
        let newv = oldv - work_done;
        if (newv > 0) {
            SchedulingDecision::Schedule
        } else {
            SchedulingDecision::NoWork
        }
    }
}

// The compiler gets stuck into recursive loop trying to figure this out itself
//unsafe impl<C: ComponentDefinition + Sized> Send for Component<C> {}
//unsafe impl<C: ComponentDefinition + Sized> Sync for Component<C> {}

unsafe impl Send for ComponentCore {}
unsafe impl Sync for ComponentCore {}

#[cfg(test)]
mod tests {
    use super::*;

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
        let system = KompicsSystem::default();
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
}

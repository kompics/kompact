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
use std::cell::UnsafeCell;

use super::*;
use crate::{actors::TypedMsgQueue, messaging::PathResolvable, supervision::*};
use bytes::{BufMut, BytesMut};
use crate::net::buffer::{ChunkLease, BufferChunk, DecodeBuffer, EncodeBuffer};
use crate::net::buffer_pool::BufferPool;

pub trait CoreContainer: Send + Sync {
    fn id(&self) -> &Uuid;
    fn core(&self) -> &ComponentCore;
    fn execute(&self) -> ();

    fn control_port(&self) -> ProvidedRef<ControlPort>;
    fn system(&self) -> KompactSystem {
        self.core().system().clone()
    }
    fn schedule(&self) -> ();
}

impl fmt::Debug for dyn CoreContainer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CoreContainer({})", self.id())
    }
}

pub struct Component<C: ComponentDefinition + ActorRaw + Sized + 'static> {
    core: ComponentCore,
    custom_scheduler: Option<dedicated_scheduler::DedicatedThreadScheduler>,
    definition: Mutex<C>,
    ctrl_queue: Arc<ConcurrentQueue<<ControlPort as Port>::Request>>,
    msg_queue: Arc<TypedMsgQueue<C::Message>>,
    skip: AtomicUsize,
    state: AtomicUsize,
    supervisor: Option<ProvidedRef<SupervisionPort>>, // system components don't have supervision
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

    pub fn logger(&self) -> &KompactLogger {
        &self.logger
    }

    pub fn is_faulty(&self) -> bool {
        lifecycle::is_faulty(&self.state)
    }

    pub fn is_active(&self) -> bool {
        lifecycle::is_active(&self.state)
    }

    pub fn is_destroyed(&self) -> bool {
        lifecycle::is_destroyed(&self.state)
    }

    /// Wait synchronously for this component be either destroyed or faulty.
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

impl<CD> ActorRefFactory<CD::Message> for Arc<Component<CD>>
where
    CD: ComponentDefinition + ActorRaw + 'static,
{
    fn actor_ref(&self) -> ActorRef<CD::Message> {
        let comp = Arc::downgrade(self);
        let msgq = Arc::downgrade(&self.msg_queue);
        ActorRef::new(comp, msgq)
    }
}

impl<CD> DynActorRefFactory for Arc<Component<CD>>
where
    CD: ComponentDefinition + ActorRaw + 'static,
{
    fn dyn_ref(&self) -> DynActorRef {
        let component = Arc::downgrade(self);
        let msg_queue = Arc::downgrade(&self.msg_queue);
        DynActorRef::from(component, msg_queue)
    }
}

impl<CD> ActorRefFactory<CD::Message> for CD
where
    CD: ComponentDefinition + ActorRaw + 'static,
{
    fn actor_ref(&self) -> ActorRef<CD::Message> {
        self.ctx().actor_ref()
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
        PathResolvable::ActorId(self.ctx().id())
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
                error!(self.logger, "Component panicked with: {:?}", e);
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

struct ComponentContextInner<CD: ComponentDefinition + ActorRaw + Sized + 'static> {
    timer_manager: TimerManager<CD>,
    component: Weak<Component<CD>>,
    logger: KompactLogger,
    actor_ref: ActorRef<CD::Message>,
    buffer: Option<EncodeBuffer>,
}

impl<CD: ComponentDefinition + ActorRaw + Sized + 'static> ComponentContextInner<CD> {

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
            buffer: None,
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

    pub fn log(&self) -> &KompactLogger {
        &self.inner_ref().logger
    }

    pub(crate) fn timer_manager_mut(&mut self) -> &mut TimerManager<CD> {
        &mut self.inner_mut().timer_manager
    }

    pub fn component(&self) -> Arc<dyn CoreContainer> {
        match self.inner_ref().component.upgrade() {
            Some(ac) => ac,
            None => panic!("Component already deallocated!"),
        }
    }

    pub fn system(&self) -> KompactSystem {
        self.component().system()
    }

    pub fn dispatcher_ref(&self) -> DispatcherRef {
        self.system().dispatcher_ref()
    }

    pub fn deadletter_ref(&self) -> ActorRef<Never> {
        self.system().actor_ref()
    }

    pub fn id(&self) -> Uuid {
        self.component().id().clone()
    }

    pub fn suicide(&self) -> () {
        self.component().control_port().enqueue(ControlEvent::Kill);
    }

    pub fn initialize_pool(&mut self) -> () {
        self.inner_mut().buffer = Some(EncodeBuffer::new());
    }

    pub fn get_buffer(&mut self, size: usize) -> Option<ChunkLease> {
        //self.inner_mut().get_buffer(size)
        if let Some(buffer) = &mut self.inner_mut().buffer {
            buffer.get_buffer(size)
        } else {
            panic!("Buffer not initialized!")
        }
    }
}

impl<CD> ActorRefFactory<CD::Message> for ComponentContext<CD>
where
    CD: ComponentDefinition + ActorRaw + Sized + 'static,
{
    fn actor_ref(&self) -> ActorRef<CD::Message> {
        self.inner_ref().actor_ref.clone()
    }
}

impl<CD> ActorPathFactory for ComponentContext<CD>
where
    CD: ComponentDefinition + ActorRaw + Sized + 'static,
{
    fn actor_path(&self) -> ActorPath {
        let id = self.id();
        ActorPath::Unique(UniquePath::with_system(self.system().system_path(), id))
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

pub trait ComponentLogging {
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

pub trait Provide<P: Port + 'static> {
    fn handle(&mut self, event: P::Request) -> ();
}

pub trait Require<P: Port + 'static> {
    fn handle(&mut self, event: P::Indication) -> ();
}

pub trait ProvideRef<P: Port + 'static> {
    fn provided_ref(&mut self) -> ProvidedRef<P>;
    fn connect_to_required(&mut self, req: RequiredRef<P>) -> ();
}
pub trait RequireRef<P: Port + 'static> {
    fn required_ref(&mut self) -> RequiredRef<P>;
    fn connect_to_provided(&mut self, prov: ProvidedRef<P>) -> ();
}

pub trait LockingProvideRef<P: Port + 'static> {
    fn provided_ref(&self) -> ProvidedRef<P>;
    fn connect_to_required(&self, req: RequiredRef<P>) -> ();
}
pub trait LockingRequireRef<P: Port + 'static> {
    fn required_ref(&self) -> RequiredRef<P>;
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

pub enum SchedulingDecision {
    Schedule,
    AlreadyScheduled,
    NoWork,
}

pub struct ComponentCore {
    id: Uuid,
    system: KompactSystem,
    work_count: AtomicUsize,
    component: UnsafeCell<Weak<dyn CoreContainer>>,
}

impl ComponentCore {
    pub fn with<CC: CoreContainer + Sized + 'static>(system: KompactSystem) -> ComponentCore {
        let weak_sized = Weak::<CC>::new();
        let weak = weak_sized as Weak<dyn CoreContainer>;
        ComponentCore {
            id: Uuid::new_v4(),
            system,
            work_count: AtomicUsize::new(0),
            component: UnsafeCell::new(weak),
        }
    }

    pub fn system(&self) -> &KompactSystem {
        &self.system
    }

    pub fn id(&self) -> &Uuid {
        &self.id
    }

    pub(crate) unsafe fn set_component(&self, c: Arc<dyn CoreContainer>) -> () {
        let component_mut = self.component.get();
        *component_mut = Arc::downgrade(&c);
    }

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
    use crate::prelude::*;

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
}

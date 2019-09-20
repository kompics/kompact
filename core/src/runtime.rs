use super::*;

use crate::{
    messaging::{
        DispatchEnvelope,
        MsgEnvelope,
        PathResolvable,
        RegistrationEnvelope,
        RegistrationError,
    },
    supervision::{ComponentSupervisor, ListenEvent, SupervisionPort, SupervisorMsg},
};
use executors::*;
use oncemutex::{OnceMutex, OnceMutexGuard};
use std::{
    clone::Clone,
    fmt::{Debug, Formatter, Result as FmtResult},
    rc::Rc,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
        Mutex,
        Once,
    },
};

static GLOBAL_RUNTIME_COUNT: AtomicUsize = AtomicUsize::new(0);

fn default_runtime_label() -> String {
    let runtime_count = GLOBAL_RUNTIME_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
    format!("kompact-runtime-{}", runtime_count)
}

static mut DEFAULT_ROOT_LOGGER: Option<KompactLogger> = None;
static DEFAULT_ROOT_LOGGER_INIT: Once = Once::new();

fn default_logger() -> &'static KompactLogger {
    unsafe {
        DEFAULT_ROOT_LOGGER_INIT.call_once(|| {
            let decorator = slog_term::TermDecorator::new().stdout().build();
            let drain = slog_term::FullFormat::new(decorator).build().fuse();
            let drain = slog_async::Async::new(drain).chan_size(1024).build().fuse();
            DEFAULT_ROOT_LOGGER = Some(slog::Logger::root_typed(
                Arc::new(drain),
                o!(
                "location" => slog::PushFnValue(|r: &slog::Record<'_>, ser: slog::PushFnValueSerializer<'_>| {
                    ser.emit(format_args!("{}:{}", r.file(), r.line()))
                })
                        ),
            ));
        });
        match DEFAULT_ROOT_LOGGER {
            Some(ref l) => l,
            None => unreachable!(),
        }
    }
}

type SchedulerBuilder = dyn Fn(usize) -> Box<dyn Scheduler>;

// impl Debug for SchedulerBuilder {
//     fn fmt(&self, f: &mut Formatter) -> Result<(), Error> {
//         write!(f, "<function>")
//     }
// }

type SCBuilder = dyn Fn(&KompactSystem, Promise<()>, Promise<()>) -> Box<dyn SystemComponents>;

// impl Debug for SCBuilder {
//     fn fmt(&self, f: &mut Formatter) -> FmtResult {
//         write!(f, "<function>")
//     }
// }

type TimerBuilder = dyn Fn() -> Box<dyn TimerComponent>;

// impl Debug for TimerBuilder {
//     fn fmt(&self, f: &mut Formatter) -> FmtResult {
//         write!(f, "<function>")
//     }
// }

#[derive(Debug, PartialEq, Clone)]
pub enum KompactError {
    Poisoned,
}

#[derive(Clone)]
pub struct KompactConfig {
    label: String,
    throughput: usize,
    msg_priority: f32,
    threads: usize,
    timer_builder: Rc<TimerBuilder>,
    scheduler_builder: Rc<SchedulerBuilder>,
    sc_builder: Rc<SCBuilder>,
    root_logger: Option<KompactLogger>,
}

impl Debug for KompactConfig {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
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
            root_logger={:?}
        }}",
            self.label, self.throughput, self.msg_priority, self.threads, self.root_logger
        )
    }
}

impl KompactConfig {
    pub fn new() -> KompactConfig {
        KompactConfig {
            label: default_runtime_label(),
            throughput: 2,
            msg_priority: 0.5,
            threads: 1,
            timer_builder: Rc::new(|| DefaultTimer::new_timer_component()),
            scheduler_builder: Rc::new(|t| {
                ExecutorScheduler::from(crossbeam_workstealing_pool::small_pool(t))
            }),
            sc_builder: Rc::new(|sys, dead_prom, disp_prom| {
                Box::new(DefaultComponents::new(sys, dead_prom, disp_prom))
            }),
            root_logger: None,
        }
    }

    pub fn label(&mut self, s: String) -> &mut Self {
        self.label = s;
        self
    }

    pub fn throughput(&mut self, n: usize) -> &mut Self {
        self.throughput = n;
        self
    }

    pub fn msg_priority(&mut self, r: f32) -> &mut Self {
        self.msg_priority = r;
        self
    }

    pub fn threads(&mut self, n: usize) -> &mut Self {
        self.threads = n;
        self
    }

    pub fn scheduler<E, F>(&mut self, f: F) -> &mut Self
    where
        E: Executor + Sync + 'static,
        F: Fn(usize) -> E + 'static,
    {
        let sb = move |t: usize| ExecutorScheduler::from(f(t));
        self.scheduler_builder = Rc::new(sb);
        self
    }

    pub fn timer<T, F>(&mut self, f: F) -> &mut Self
    where
        T: TimerComponent + 'static,
        F: Fn() -> Box<dyn TimerComponent> + 'static,
    {
        self.timer_builder = Rc::new(f);
        self
    }

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
        FB: Fn(Promise<()>) -> B + 'static,
        FC: Fn(Promise<()>) -> C + 'static,
    {
        let sb = move |system: &KompactSystem, dead_prom: Promise<()>, disp_prom: Promise<()>| {
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
        FB: Fn(Promise<()>) -> B + 'static,
        FC: Fn(Promise<()>) -> C + 'static,
    {
        let sb = move |system: &KompactSystem, dead_prom: Promise<()>, disp_prom: Promise<()>| {
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

    pub fn logger(&mut self, logger: KompactLogger) -> &mut Self {
        self.root_logger = Some(logger);
        self
    }

    pub fn build(self) -> Result<KompactSystem, KompactError> {
        KompactSystem::new(self)
    }

    fn max_messages(&self) -> usize {
        let tpf = self.throughput as f32;
        let mmf = tpf * self.msg_priority;
        assert!(mmf >= 0.0, "msg_priority can not be negative!");
        let mm = mmf as usize;
        mm
    }
}

impl Default for KompactConfig {
    fn default() -> Self {
        let threads = num_cpus::get();
        let scheduler_builder: Rc<SchedulerBuilder> = if threads <= 32 {
            Rc::new(|t| ExecutorScheduler::from(crossbeam_workstealing_pool::small_pool(t)))
        } else if threads <= 64 {
            Rc::new(|t| ExecutorScheduler::from(crossbeam_workstealing_pool::large_pool(t)))
        } else {
            Rc::new(|t| ExecutorScheduler::from(crossbeam_workstealing_pool::dyn_pool(t)))
        };
        KompactConfig {
            label: default_runtime_label(),
            throughput: 50,
            msg_priority: 0.5,
            threads,
            timer_builder: Rc::new(|| DefaultTimer::new_timer_component()),
            scheduler_builder,
            sc_builder: Rc::new(|sys, dead_prom, disp_prom| {
                Box::new(DefaultComponents::new(sys, dead_prom, disp_prom))
            }),
            root_logger: None,
        }
    }
}

#[derive(Clone)]
pub struct KompactSystem {
    inner: Arc<KompactRuntime>,
    scheduler: Box<dyn Scheduler>,
}

impl KompactSystem {
    /// Use the [build](KompactConfig::build) method instead.
    pub(crate) fn new(conf: KompactConfig) -> Result<Self, KompactError> {
        let scheduler = (*conf.scheduler_builder)(conf.threads);
        let sc_builder = conf.sc_builder.clone();
        let runtime = Arc::new(KompactRuntime::new(conf));
        let sys = KompactSystem {
            inner: runtime,
            scheduler,
        };
        let (dead_prom, dead_f) = utils::promise();
        let (disp_prom, disp_f) = utils::promise();
        let system_components = (*sc_builder)(&sys, dead_prom, disp_prom); //(*conf.sc_builder)(&sys);
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

    pub fn logger(&self) -> &KompactLogger {
        &self.inner.logger
    }

    pub(crate) fn poison(&self) {
        self.inner.poison();
        self.scheduler.poison();
    }

    /// Create a new component.
    ///
    /// New components are not started automatically.
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
        return c;
    }

    /// Use this to create system components!
    ///
    /// During system initialisation the supervisor is not available, yet,
    /// so normal create calls will panic!
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
        return c;
    }

    /// Create a new component, which runs on its own dedicated thread.
    ///
    /// New components are not started automatically.
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
        promise.fulfill(c.clone()).expect("Should accept component");
        return c;
    }

    /// Create a new component, which runs on its own dedicated thread and is pinned to certain CPU core.
    ///
    /// New components are not started automatically.
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
        promise.fulfill(c.clone()).expect("Should accept component");
        return c;
    }

    /// Use this to create system components, which runs on their own dedicated thread.
    ///
    /// During system initialisation the supervisor is not available, yet,
    /// so normal create calls will panic!
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
        promise.fulfill(c.clone()).expect("Should accept component");
        return c;
    }

    pub fn create_and_register<C, F>(
        &self,
        f: F,
    ) -> (
        Arc<Component<C>>,
        Future<Result<ActorPath, RegistrationError>>,
    )
    where
        F: FnOnce() -> C,
        C: ComponentDefinition + 'static,
    {
        self.inner.assert_active();
        let c = self.create(f);
        let id = c.core().id().clone();
        let id_path = PathResolvable::ActorId(id);
        let r = self.inner.register_by_path(&c, id_path);
        (c, r)
    }

    /// Instantiates the component, registers it with the system dispatcher,
    /// and starts its lifecycle.
    pub fn create_and_start<C, F>(&self, f: F) -> Arc<Component<C>>
    where
        F: FnOnce() -> C,
        C: ComponentDefinition + 'static,
    {
        self.inner.assert_active();
        let c = self.create(f);
        let path = PathResolvable::ActorId(c.core().id().clone());
        self.inner.register_by_path(&c, path);
        self.start(&c);
        c
    }

    /// Attempts to register the provided component with a human-readable alias.
    ///
    /// # Returns
    /// A [Future](kompact::utils::Future) which resolves to an error if the alias is not unique,
    /// and the newly created [ActorPath](kompact::actors::ActorPath) if successful.
    pub fn register_by_alias<C, A>(
        &self,
        c: &Arc<Component<C>>,
        alias: A,
    ) -> Future<Result<ActorPath, RegistrationError>>
    where
        C: ComponentDefinition + 'static,
        A: Into<String>,
    {
        self.inner.assert_active();
        self.inner.register_by_alias(c, alias.into())
    }

    pub fn start<C>(&self, c: &Arc<Component<C>>) -> ()
    where
        C: ComponentDefinition + 'static,
    {
        self.inner.assert_not_poisoned();
        c.enqueue_control(ControlEvent::Start);
    }

    pub fn start_notify<C>(&self, c: &Arc<Component<C>>) -> Future<()>
    where
        C: ComponentDefinition + 'static,
    {
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

    pub fn stop<C>(&self, c: &Arc<Component<C>>) -> ()
    where
        C: ComponentDefinition + 'static,
    {
        self.inner.assert_active();
        c.enqueue_control(ControlEvent::Stop);
    }

    pub fn stop_notify<C>(&self, c: &Arc<Component<C>>) -> Future<()>
    where
        C: ComponentDefinition + 'static,
    {
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

    pub fn kill<C>(&self, c: Arc<Component<C>>) -> ()
    where
        C: ComponentDefinition + 'static,
    {
        self.inner.assert_active();
        c.enqueue_control(ControlEvent::Kill);
    }

    pub fn kill_notify<C>(&self, c: Arc<Component<C>>) -> Future<()>
    where
        C: ComponentDefinition + 'static,
    {
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

    pub fn trigger_i<P: Port + 'static>(&self, msg: P::Indication, port: &RequiredRef<P>) {
        self.inner.assert_active();
        port.enqueue(msg);
    }

    pub fn trigger_r<P: Port + 'static>(&self, msg: P::Request, port: &ProvidedRef<P>) {
        self.inner.assert_active();
        port.enqueue(msg);
    }

    pub fn throughput(&self) -> usize {
        self.inner.throughput
    }

    //    pub fn msg_priority(&self) -> f32 {
    //        self.inner.msg_priority
    //    }

    pub fn max_messages(&self) -> usize {
        self.inner.max_messages
    }

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

    pub fn shutdown(self) -> Result<(), String> {
        self.inner.assert_active();
        self.inner.shutdown(&self)?;
        self.scheduler.shutdown()?;
        Ok(())
    }

    pub fn system_path(&self) -> SystemPath {
        self.inner.assert_active();
        self.inner.system_path()
    }

    pub fn actor_path_for<C>(&self, component: &Arc<Component<C>>) -> ActorPath
    where
        C: ComponentDefinition + 'static,
    {
        let id = *component.id();
        ActorPath::Unique(UniquePath::with_system(self.system_path(), id))
    }

    pub(crate) fn supervision_port(&self) -> ProvidedRef<SupervisionPort> {
        self.inner.supervision_port()
    }
}

impl ActorRefFactory<Never> for KompactSystem {
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

pub trait SystemComponents: Send + Sync {
    fn deadletter_ref(&self) -> ActorRef<Never>;
    fn dispatcher_ref(&self) -> DispatcherRef;
    fn system_path(&self) -> SystemPath;
    fn start(&self, _system: &KompactSystem) -> ();
    fn stop(&self, _system: &KompactSystem) -> ();
}

pub trait TimerComponent: TimerRefFactory + Send + Sync {
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

//#[derive(Clone)]
struct KompactRuntime {
    label: String,
    throughput: usize,
    max_messages: usize,
    timer: Box<dyn TimerComponent>,
    internal_components: OnceMutex<Option<InternalComponents>>,
    logger: KompactLogger,
    state: AtomicUsize,
}

// Moved into default config
// impl Default for KompactRuntime {
//     fn default() -> Self {
//         let label = default_runtime_label();
//         KompactRuntime {
//             label: label.clone(),
//             throughput: 50,
//             max_messages: 25,
//             timer: DefaultTimer::new_timer_component(),
//             internal_components: OnceMutex::new(None),
//             logger: default_logger().new(o!("system" => label)),
//             state: lifecycle::initial_state(),
//         }
//     }
// }

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
        path: PathResolvable,
    ) -> Future<Result<ActorPath, RegistrationError>>
    where
        D: DynActorRefFactory,
    {
        debug!(self.logger(), "Requesting actor registration at {:?}", path);
        let (promise, future) = utils::promise();
        let dispatcher = self.dispatcher_ref();
        let envelope = MsgEnvelope::Typed(DispatchEnvelope::Registration(
            RegistrationEnvelope::Register(actor_ref.dyn_ref(), path, Some(promise)),
        ));
        dispatcher.enqueue(envelope);
        future
    }

    /// Registers an actor with an alias at the dispatcher
    fn register_by_alias<D>(
        &self,
        actor_ref: &D,
        alias: String,
    ) -> Future<Result<ActorPath, RegistrationError>>
    where
        D: DynActorRefFactory,
    {
        debug!(
            self.logger(),
            "Requesting actor alias registration for {:?}", alias
        );
        let path = PathResolvable::Alias(alias);
        self.register_by_path(actor_ref, path)
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

    pub fn shutdown(&self, system: &KompactSystem) -> Result<(), String> {
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

impl Debug for KompactRuntime {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(f, "KompactRuntime({})", self.label)
    }
}

pub trait Scheduler: Send + Sync {
    fn schedule(&self, c: Arc<dyn CoreContainer>) -> ();
    fn shutdown_async(&self) -> ();
    fn shutdown(&self) -> Result<(), String>;
    fn box_clone(&self) -> Box<dyn Scheduler>;
    fn poison(&self) -> ();
}

impl Clone for Box<dyn Scheduler> {
    fn clone(&self) -> Self {
        (*self).box_clone()
    }
}

#[derive(Clone)]
struct ExecutorScheduler<E>
where
    E: Executor + Sync,
{
    exec: E,
}

impl<E: Executor + Sync + 'static> ExecutorScheduler<E> {
    fn with(exec: E) -> ExecutorScheduler<E> {
        ExecutorScheduler { exec }
    }

    fn from(exec: E) -> Box<dyn Scheduler> {
        Box::new(ExecutorScheduler::with(exec))
    }
}

impl<E: Executor + Sync + 'static> Scheduler for ExecutorScheduler<E> {
    fn schedule(&self, c: Arc<dyn CoreContainer>) -> () {
        self.exec.execute(move || {
            c.execute();
        });
    }

    fn shutdown_async(&self) -> () {
        self.exec.shutdown_async()
    }

    fn shutdown(&self) -> Result<(), String> {
        self.exec.shutdown_borrowed()
    }

    fn box_clone(&self) -> Box<dyn Scheduler> {
        Box::new(self.clone())
    }

    fn poison(&self) -> () {
        self.exec.shutdown_async();
    }
}

// struct PanicGuard(Arc<CoreContainer>);

// impl Deref for PanicGuard {
//     type Target = Arc<CoreContainer>;

//     fn deref(&self) -> &Arc<CoreContainer> {
//         &self.0
//     }
// }

// impl Drop for Inner {
//     fn drop(&mut self) {
//         if thread::panicking() {
//             self.0.set_fault()
//         }
//     }
// }

use super::*;

use executors::*;
use messaging::RegistrationError;
use messaging::{DispatchEnvelope, MsgEnvelope, PathResolvable, RegistrationEnvelope};
use oncemutex::{OnceMutex, OnceMutexGuard};
use std::clone::Clone;
use std::fmt::{Debug, Error, Formatter, Result as FmtResult};
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering, ATOMIC_USIZE_INIT};
use std::sync::{Arc, Mutex};
use std::sync::{Once, ONCE_INIT};
use supervision::{ComponentSupervisor, ListenEvent, SupervisionPort, SupervisorMsg};

static GLOBAL_RUNTIME_COUNT: AtomicUsize = ATOMIC_USIZE_INIT;

fn default_runtime_label() -> String {
    let runtime_count = GLOBAL_RUNTIME_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
    format!("kompics-runtime-{}", runtime_count)
}

static mut DEFAULT_ROOT_LOGGER: Option<KompicsLogger> = None;
static DEFAULT_ROOT_LOGGER_INIT: Once = ONCE_INIT;

fn default_logger() -> &'static KompicsLogger {
    unsafe {
        DEFAULT_ROOT_LOGGER_INIT.call_once(|| {
            let decorator = slog_term::TermDecorator::new().build();
            let drain = slog_term::FullFormat::new(decorator).build().fuse();
            let drain = slog_async::Async::new(drain).build().fuse();
            DEFAULT_ROOT_LOGGER = Some(slog::Logger::root_typed(Arc::new(drain), o!()));
        });
        match DEFAULT_ROOT_LOGGER {
            Some(ref l) => l,
            None => unreachable!(),
        }
    }
}

type SchedulerBuilder = Fn(usize) -> Box<Scheduler>;

// impl Debug for SchedulerBuilder {
//     fn fmt(&self, f: &mut Formatter) -> Result<(), Error> {
//         write!(f, "<function>")
//     }
// }

type SCBuilder = Fn(&KompicsSystem) -> Box<SystemComponents>;

// impl Debug for SCBuilder {
//     fn fmt(&self, f: &mut Formatter) -> FmtResult {
//         write!(f, "<function>")
//     }
// }

type TimerBuilder = Fn() -> Box<TimerComponent>;

// impl Debug for TimerBuilder {
//     fn fmt(&self, f: &mut Formatter) -> FmtResult {
//         write!(f, "<function>")
//     }
// }

#[derive(Clone)]
pub struct KompicsConfig {
    label: String,
    throughput: usize,
    msg_priority: f32,
    threads: usize,
    timer_builder: Rc<TimerBuilder>,
    scheduler_builder: Rc<SchedulerBuilder>,
    sc_builder: Rc<SCBuilder>,
    root_logger: Option<KompicsLogger>,
}

impl Debug for KompicsConfig {
    fn fmt(&self, f: &mut Formatter) -> FmtResult {
        write!(f, "KompicsConfig{{
            label={},
            throughput={},
            msg_priority={},
            threads={},
            timer_builder=<function>,
            scheduler_builder=<function>,
            sc_builder=<function>,
            root_logger={:?}
        }}", 
            self.label, self.throughput, self.msg_priority, self.threads, self.root_logger)
    }
}

impl KompicsConfig {
    pub fn new() -> KompicsConfig {
        KompicsConfig {
            label: default_runtime_label(),
            throughput: 1,
            msg_priority: 0.5,
            threads: 1,
            timer_builder: Rc::new(|| DefaultTimer::new_timer_component()),
            scheduler_builder: Rc::new(|t| {
                ExecutorScheduler::from(crossbeam_channel_pool::ThreadPool::new(t))
            }),
            sc_builder: Rc::new(|sys| Box::new(DefaultComponents::new(sys))),
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
        F: Fn() -> Box<TimerComponent> + 'static,
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
        B: ComponentDefinition + Sized + 'static,
        C: ComponentDefinition + Sized + 'static + Dispatcher,
        FB: Fn() -> B + 'static,
        FC: Fn() -> C + 'static,
    {
        let sb = move |system: &KompicsSystem| {
            let deadletter_box = system.create_unsupervised(&deadletter_fn);
            let dispatcher = system.create_unsupervised(&dispatcher_fn);

            let cc = CustomComponents {
                deadletter_box,
                dispatcher,
            };
            Box::new(cc) as Box<SystemComponents>
        };
        self.sc_builder = Rc::new(sb);
        self
    }

    pub fn logger(&mut self, logger: KompicsLogger) -> &mut Self {
        self.root_logger = Some(logger);
        self
    }

    fn max_messages(&self) -> usize {
        let tpf = self.throughput as f32;
        let mmf = tpf * self.msg_priority;
        assert!(mmf >= 0.0, "msg_priority can not be negative!");
        let mm = mmf as usize;
        mm
    }
}

#[derive(Clone)]
pub struct KompicsSystem {
    inner: Arc<KompicsRuntime>,
    scheduler: Box<Scheduler>,
}

impl Default for KompicsSystem {
    fn default() -> Self {
        let scheduler =
            ExecutorScheduler::from(crossbeam_workstealing_pool::ThreadPool::new(num_cpus::get()));
        let runtime = Arc::new(KompicsRuntime::default());
        let sys = KompicsSystem {
            inner: runtime,
            scheduler,
        };
        let system_components = Box::new(DefaultComponents::new(&sys));
        let supervisor = sys.create_unsupervised(ComponentSupervisor::new);
        let ic = InternalComponents::new(supervisor, system_components);
        sys.inner.set_internal_components(ic);
        sys.inner.start_internal_components(&sys);
        sys
    }
}

impl KompicsSystem {
    pub fn new(conf: KompicsConfig) -> Self {
        let scheduler = (*conf.scheduler_builder)(conf.threads);
        let sc_builder = conf.sc_builder.clone();
        let runtime = Arc::new(KompicsRuntime::new(conf));
        let sys = KompicsSystem {
            inner: runtime,
            scheduler,
        };
        let system_components = (*sc_builder)(&sys); //(*conf.sc_builder)(&sys);
        let supervisor = sys.create_unsupervised(ComponentSupervisor::new);
        let ic = InternalComponents::new(supervisor, system_components);
        sys.inner.set_internal_components(ic);
        sys.inner.start_internal_components(&sys);
        sys
    }

    pub fn schedule(&self, c: Arc<CoreContainer>) -> () {
        self.scheduler.schedule(c);
    }

    pub fn logger(&self) -> &KompicsLogger {
        &self.inner.logger
    }

    /// Create a new component.
    ///
    /// New components are not started automatically.
    pub fn create<C, F>(&self, f: F) -> Arc<Component<C>>
    where
        F: FnOnce() -> C,
        C: ComponentDefinition + 'static,
    {
        let c = Arc::new(Component::new(self.clone(), f(), self.supervision_port()));
        {
            let mut cd = c.definition().lock().unwrap();
            let cc: Arc<CoreContainer> = c.clone() as Arc<CoreContainer>;
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
        {
            let mut cd = c.definition().lock().unwrap();
            cd.setup(c.clone());
            let cc: Arc<CoreContainer> = c.clone() as Arc<CoreContainer>;
            c.core().set_component(cc);
        }
        return c;
    }

    pub fn create_and_register<C, F>(&self, f: F) -> Arc<Component<C>>
    where
        F: FnOnce() -> C,
        C: ComponentDefinition + 'static,
    {
        let c = self.create(f);
        let id = c.core().id().clone();
        let id_path = PathResolvable::ActorId(id);
        let actor = c.actor_ref();
        self.inner.register_by_path(actor, id_path);
        c
    }

    /// Instantiates the component, registers it with the system dispatcher,
    /// and starts its lifecycle.
    pub fn create_and_start<C, F>(&self, f: F) -> Arc<Component<C>>
    where
        F: FnOnce() -> C,
        C: ComponentDefinition + 'static,
    {
        let c = self.create(f);
        let path = PathResolvable::ActorId(c.core().id().clone());
        let actor = c.actor_ref();
        self.inner.register_by_path(actor, path);
        self.start(&c);
        c
    }

    /// Attempts to register the provided component with a human-readable alias.
    ///
    /// # Returns
    /// A [Future](kompics::utils::Future) which resolves to an error if the alias is not unique,
    /// and a unit () if successful.
    pub fn register_by_alias<C, A>(
        &self,
        c: &Arc<Component<C>>,
        alias: A,
    ) -> Future<Result<(), RegistrationError>>
    where
        C: ComponentDefinition + 'static,
        A: Into<String>,
    {
        let actor = c.actor_ref();
        self.inner.register_by_alias(actor, alias.into())
    }

    pub fn start<C>(&self, c: &Arc<Component<C>>) -> ()
    where
        C: ComponentDefinition + 'static,
    {
        c.enqueue_control(ControlEvent::Start);
    }

    pub fn start_notify<C>(&self, c: &Arc<Component<C>>) -> Future<()>
    where
        C: ComponentDefinition + 'static,
    {
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
        c.enqueue_control(ControlEvent::Stop);
    }

    pub fn stop_notify<C>(&self, c: &Arc<Component<C>>) -> Future<()>
    where
        C: ComponentDefinition + 'static,
    {
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
        c.enqueue_control(ControlEvent::Kill);
    }

    pub fn kill_notify<C>(&self, c: Arc<Component<C>>) -> Future<()>
    where
        C: ComponentDefinition + 'static,
    {
        let (p, f) = utils::promise();
        let amp = Arc::new(Mutex::new(p));
        self.supervision_port().enqueue(SupervisorMsg::Listen(
            amp,
            ListenEvent::Destroyed(c.id().clone()),
        ));
        c.enqueue_control(ControlEvent::Kill);
        f
    }

    pub fn trigger_i<P: Port + 'static>(&self, msg: P::Indication, port: RequiredRef<P>) {
        port.enqueue(msg);
    }

    pub fn trigger_r<P: Port + 'static>(&self, msg: P::Request, port: ProvidedRef<P>) {
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

    // TODO add shutdown_on_idle() to block the current thread until the system has nothing more to do
    pub fn shutdown(self) -> Result<(), String> {
        self.scheduler.shutdown()?;
        self.inner.shutdown()?;
        Ok(())
    }

    pub fn system_path(&self) -> SystemPath {
        self.inner.system_path()
    }

    pub(crate) fn supervision_port(&self) -> ProvidedRef<SupervisionPort> {
        self.inner.supervision_port()
    }
}

impl ActorRefFactory for KompicsSystem {
    fn actor_ref(&self) -> ActorRef {
        self.inner.deadletter_ref()
    }
}

impl Dispatching for KompicsSystem {
    fn dispatcher_ref(&self) -> ActorRef {
        self.inner.dispatcher_ref()
    }
}

impl ActorSource for KompicsSystem {
    fn path_resolvable(&self) -> PathResolvable {
        PathResolvable::System
    }
}

impl TimerRefFactory for KompicsSystem {
    fn timer_ref(&self) -> timer::TimerRef {
        self.inner.timer_ref()
    }
}

pub trait SystemComponents: Send + Sync {
    fn deadletter_ref(&self) -> ActorRef;
    fn dispatcher_ref(&self) -> ActorRef;
    fn system_path(&self) -> SystemPath;
    fn start(&self, &KompicsSystem) -> ();
}

pub trait TimerComponent: TimerRefFactory + Send + Sync {
    fn shutdown(&self) -> Result<(), String>;
}

struct InternalComponents {
    supervisor: Arc<Component<ComponentSupervisor>>,
    supervision_port: ProvidedRef<SupervisionPort>,
    system_components: Box<SystemComponents>,
}

impl InternalComponents {
    fn new(
        supervisor: Arc<Component<ComponentSupervisor>>,
        system_components: Box<SystemComponents>,
    ) -> InternalComponents {
        let supervision_port = supervisor.on_definition(|s| s.supervision.share());
        InternalComponents {
            supervisor,
            supervision_port,
            system_components,
        }
    }

    fn start(&self, system: &KompicsSystem) -> () {
        self.system_components.start(system);
        system.start(&self.supervisor);
    }

    fn deadletter_ref(&self) -> ActorRef {
        self.system_components.deadletter_ref()
    }
    fn dispatcher_ref(&self) -> ActorRef {
        self.system_components.dispatcher_ref()
    }
    fn system_path(&self) -> SystemPath {
        self.system_components.system_path()
    }
    fn supervision_port(&self) -> ProvidedRef<SupervisionPort> {
        self.supervision_port.clone()
    }
}

//#[derive(Clone)]
struct KompicsRuntime {
    label: String,
    throughput: usize,
    max_messages: usize,
    timer: Box<TimerComponent>,
    internal_components: OnceMutex<Option<InternalComponents>>,
    logger: KompicsLogger,
}

impl Default for KompicsRuntime {
    fn default() -> Self {
        let label = default_runtime_label();
        KompicsRuntime {
            label: label.clone(),
            throughput: 50,
            max_messages: 25,
            timer: DefaultTimer::new_timer_component(),
            internal_components: OnceMutex::new(None),
            logger: default_logger().new(o!("system" => label)),
        }
    }
}

impl KompicsRuntime {
    fn new(conf: KompicsConfig) -> Self {
        let mm = conf.max_messages();
        let logger = match conf.root_logger {
            Some(log) => log.new(o!("system" => conf.label.clone())),
            None => default_logger().new(o!("system" => conf.label.clone())),
        };
        KompicsRuntime {
            label: conf.label,
            throughput: conf.throughput,
            max_messages: mm,
            timer: (conf.timer_builder)(),
            internal_components: OnceMutex::new(None),
            logger,
        }
    }

    fn set_internal_components(&self, internal_components: InternalComponents) -> () {
        let guard_opt: Option<OnceMutexGuard<Option<InternalComponents>>> =
            self.internal_components.lock();
        if let Some(mut guard) = guard_opt {
            *guard = Some(internal_components);
        } else {
            panic!("KompicsRuntime was already initialised!");
        }
    }

    fn start_internal_components(&self, system: &KompicsSystem) -> () {
        match *self.internal_components {
            Some(ref ic) => ic.start(system),
            None => panic!("KompicsRuntime was not properly initialised!"),
        }
    }

    fn logger(&self) -> &KompicsLogger {
        &self.logger
    }

    /// Registers an actor with a path at the dispatcher
    fn register_by_path(
        &self,
        actor_ref: ActorRef,
        path: PathResolvable,
    ) -> Future<Result<(), RegistrationError>> {
        debug!(self.logger(), "Requesting actor registration at {:?}", path);
        let (promise, future) = utils::promise();
        let dispatcher = self.dispatcher_ref();
        let envelope = MsgEnvelope::Dispatch(DispatchEnvelope::Registration(
            RegistrationEnvelope::Register(actor_ref, path, Some(promise)),
        ));
        dispatcher.enqueue(envelope);
        future
    }

    /// Registers an actor with an alias at the dispatcher
    fn register_by_alias(
        &self,
        actor_ref: ActorRef,
        alias: String,
    ) -> Future<Result<(), RegistrationError>> {
        debug!(
            self.logger(),
            "Requesting actor registration for {:?}",
            alias
        );
        let path = PathResolvable::Alias(alias);
        self.register_by_path(actor_ref, path)
    }

    fn deadletter_ref(&self) -> ActorRef {
        match *self.internal_components {
            Some(ref sc) => sc.deadletter_ref(),
            None => panic!("KompicsRuntime was not properly initialised!"),
        }
    }
    fn dispatcher_ref(&self) -> ActorRef {
        match *self.internal_components {
            Some(ref sc) => sc.dispatcher_ref(),
            None => panic!("KompicsRuntime was not properly initialised!"),
        }
    }
    fn system_path(&self) -> SystemPath {
        match *self.internal_components {
            Some(ref sc) => sc.system_path(),
            None => panic!("KompicsRuntime was not properly initialised!"),
        }
    }

    fn supervision_port(&self) -> ProvidedRef<SupervisionPort> {
        match *self.internal_components {
            Some(ref ic) => ic.supervision_port(),
            None => panic!("KompicsRuntime was not properly initialised!"),
        }
    }

    fn timer_ref(&self) -> timer::TimerRef {
        self.timer.timer_ref()
    }

    pub fn shutdown(&self) -> Result<(), String> {
        self.timer.shutdown()
    }
}

impl Debug for KompicsRuntime {
    fn fmt(&self, f: &mut Formatter) -> FmtResult {
        write!(f, "KompicsRuntime({})", self.label)
    }
}

pub trait Scheduler: Send + Sync {
    fn schedule(&self, c: Arc<CoreContainer>) -> ();
    fn shutdown_async(&self) -> ();
    fn shutdown(&self) -> Result<(), String>;
    fn box_clone(&self) -> Box<Scheduler>;
}

impl Clone for Box<Scheduler> {
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
    fn from(exec: E) -> Box<Scheduler> {
        Box::new(ExecutorScheduler::with(exec))
    }
}

impl<E: Executor + Sync + 'static> Scheduler for ExecutorScheduler<E> {
    fn schedule(&self, c: Arc<CoreContainer>) -> () {
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
    fn box_clone(&self) -> Box<Scheduler> {
        Box::new(self.clone())
    }
}

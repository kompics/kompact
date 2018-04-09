
use super::*;

use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicUsize, Ordering, ATOMIC_USIZE_INIT};
use std::rc::Rc;
use std::fmt::{Debug, Formatter, Error};
use oncemutex::OnceMutex;
use executors::*;

static GLOBAL_RUNTIME_COUNT: AtomicUsize = ATOMIC_USIZE_INIT;

fn default_runtime_label() -> String {
    let runtime_count = GLOBAL_RUNTIME_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
    format!("kompics-runtime-{}", runtime_count)
}

type SchedulerBuilder = Fn(usize) -> Box<Scheduler>;

impl Debug for SchedulerBuilder {
    fn fmt(&self, f: &mut Formatter) -> Result<(), Error> {
        write!(f, "<function>")
    }
}

type SCBuilder = Fn(&KompicsSystem) -> Box<SystemComponents>;

impl Debug for SCBuilder {
    fn fmt(&self, f: &mut Formatter) -> Result<(), Error> {
        write!(f, "<function>")
    }
}


#[derive(Clone, Debug)]
pub struct KompicsConfig {
    label: String,
    throughput: usize,
    msg_priority: f32,
    threads: usize,
    scheduler_builder: Rc<SchedulerBuilder>,
    sc_builder: Rc<SCBuilder>,
}

impl KompicsConfig {
    pub fn new() -> KompicsConfig {
        KompicsConfig {
            label: default_runtime_label(),
            throughput: 1,
            msg_priority: 0.5,
            threads: 1,
            scheduler_builder: Rc::new(|t| {
                ExecutorScheduler::from(crossbeam_channel_pool::ThreadPool::new(t))
            }),
            sc_builder: Rc::new(|sys| Box::new(DefaultComponents::new(sys))),
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
        E: Executor + 'static,
        F: Fn(usize) -> E + 'static,
    {
        let sb = move |t: usize| ExecutorScheduler::from(f(t));
        self.scheduler_builder = Rc::new(sb);
        self
    }

    pub fn system_components<B, C, FB, FC>(&mut self, fb: FB, fc: FC) -> &mut Self
    where
        B: ComponentDefinition + Sized + 'static,
        C: ComponentDefinition + Sized + 'static + Dispatcher,
        FB: Fn() -> B + 'static,
        FC: Fn() -> C + 'static,
    {
        let sb = move |system: &KompicsSystem| {
            let dbc = system.create(&fb);
            let ldc = system.create(&fc);
            let cc = CustomComponents {
                deadletter_box: dbc,
                dispatcher: ldc,
            };
            Box::new(cc) as Box<SystemComponents>
        };
        self.sc_builder = Rc::new(sb);
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
        let scheduler = ExecutorScheduler::from(crossbeam_workstealing_pool::ThreadPool::new(
            num_cpus::get(),
        ));
        let runtime = Arc::new(KompicsRuntime::default());
        let sys = KompicsSystem {
            inner: runtime,
            scheduler,
        };
        let sys_comps = DefaultComponents::new(&sys);
        sys.inner.set_system_components(Box::new(sys_comps));
        sys.inner.start_system_components(&sys);
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
        let sys_comps = (*sc_builder)(&sys); //(*conf.sc_builder)(&sys);
        sys.inner.set_system_components(sys_comps);
        sys.inner.start_system_components(&sys);
        sys
    }

    pub fn schedule(&self, c: Arc<CoreContainer>) -> () {
        self.scheduler.schedule(c);
    }

    pub fn create<C, F>(&self, f: F) -> Arc<Component<C>>
    where
        F: Fn() -> C,
        C: ComponentDefinition + 'static,
    {

        let c = Arc::new(Component::new(self.clone(), f()));
        {
            let mut cd = c.definition().lock().unwrap();
            cd.setup(c.clone());
            let cc: Arc<CoreContainer> = c.clone() as Arc<CoreContainer>;
            c.core().set_component(cc);
        }
        return c;
    }

    pub fn start<C>(&self, c: &Arc<Component<C>>) -> ()
    where
        C: ComponentDefinition + 'static,
    {
        c.enqueue_control(ControlEvent::Start);
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

    pub fn shutdown(self) -> Result<(), String> {
        self.scheduler.shutdown()
    }

    pub(crate) fn system_path(&self) -> SystemPath {
        self.inner.system_path()
    }
}

impl ActorRefFactory for KompicsSystem {
    fn actor_ref(&self) -> ActorRef {
        self.inner.deadletter_ref()
    }
    fn actor_path(&self) -> ActorPath {
        self.inner.deadletter_ref().actor_path()
    }
}

impl Dispatching for KompicsSystem {
    fn dispatcher_ref(&self) -> ActorRef {
        self.inner.dispatcher_ref()
    }
}

pub trait SystemComponents: Send + Sync {
    fn deadletter_ref(&self) -> ActorRef;
    fn dispatcher_ref(&self) -> ActorRef;
    fn system_path(&self) -> SystemPath;
    fn start(&self, &KompicsSystem) -> ();
}

//#[derive(Clone)]
struct KompicsRuntime {
    label: String,
    throughput: usize,
    max_messages: usize,
    system_components: OnceMutex<Option<Box<SystemComponents>>>,
}

impl KompicsRuntime {
    fn new(conf: KompicsConfig) -> Self {
        let mm = conf.max_messages();
        KompicsRuntime {
            label: conf.label,
            throughput: conf.throughput,
            max_messages: mm,
            system_components: OnceMutex::new(None),
        }
    }

    fn set_system_components(&self, system_components: Box<SystemComponents>) -> () {
        if let Some(mut guard) = self.system_components.lock() {
            *guard = Some(system_components);
        } else {
            panic!("KompicsRuntime was already initialised!");
        }
    }

    fn start_system_components(&self, system: &KompicsSystem) -> () {
        match *self.system_components {
            Some(ref sc) => sc.start(system),
            None => panic!("KompicsRuntime was not properly initialised!"),
        }
    }

    fn deadletter_ref(&self) -> ActorRef {
        match *self.system_components {
            Some(ref sc) => sc.deadletter_ref(),
            None => panic!("KompicsRuntime was not properly initialised!"),
        }
    }
    fn dispatcher_ref(&self) -> ActorRef {
        match *self.system_components {
            Some(ref sc) => sc.dispatcher_ref(),
            None => panic!("KompicsRuntime was not properly initialised!"),
        }
    }
    fn system_path(&self) -> SystemPath {
        match *self.system_components {
            Some(ref sc) => sc.system_path(),
            None => panic!("KompicsRuntime was not properly initialised!"),
        }
    }
}

impl Default for KompicsRuntime {
    fn default() -> Self {
        KompicsRuntime {
            label: default_runtime_label(),
            throughput: 50,
            max_messages: 25,
            system_components: OnceMutex::new(None),
        }
    }
}

pub trait Scheduler {
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
struct ExecutorScheduler<E: Executor> {
    exec: E,
}

impl<E: Executor + 'static> ExecutorScheduler<E> {
    fn with(exec: E) -> ExecutorScheduler<E> {
        ExecutorScheduler { exec }
    }
    fn from(exec: E) -> Box<Scheduler> {
        Box::new(ExecutorScheduler { exec })
    }
}

impl<E: Executor + 'static> Scheduler for ExecutorScheduler<E> {
    fn schedule(&self, c: Arc<CoreContainer>) -> () {
        self.exec.execute(move || {
            //println!("Executing component {}", c.id());
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

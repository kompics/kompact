
use super::*;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering, ATOMIC_USIZE_INIT};
use std::rc::Rc;
use std::fmt::{Debug, Formatter, Error};
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

#[derive(Clone, Debug)]
pub struct KompicsConfig {
    label: String,
    throughput: usize,
    threads: usize,
    scheduler_builder: Rc<SchedulerBuilder>,
}

impl KompicsConfig {
    pub fn new() -> KompicsConfig {
        KompicsConfig {
            label: default_runtime_label(),
            throughput: 1,
            threads: 1,
            scheduler_builder: Rc::new(|t| {
                ExecutorScheduler::from(crossbeam_channel_pool::ThreadPool::new(t))
            }),
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
}

#[derive(Clone)]
pub struct KompicsSystem {
    inner: Arc<KompicsRuntime>,
    scheduler: Box<Scheduler>,
}

impl Default for KompicsSystem {
    fn default() -> Self {
        KompicsSystem {
            inner: Arc::new(KompicsRuntime::default()),
            scheduler: ExecutorScheduler::from(crossbeam_workstealing_pool::ThreadPool::new(
                num_cpus::get(),
            )),
        }
    }
}

impl KompicsSystem {
    pub fn new(conf: KompicsConfig) -> Self {
        let scheduler = (*conf.scheduler_builder)(conf.threads);
        KompicsSystem {
            inner: Arc::new(KompicsRuntime::new(conf)),
            scheduler,
        }
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
            cd.setup_ports(c.clone());
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

    pub fn shutdown(self) -> Result<(), String> {
        self.scheduler.shutdown()
    }
}

#[derive(Clone)]
struct KompicsRuntime {
    label: String,
    throughput: usize,
}

impl KompicsRuntime {
    fn new(conf: KompicsConfig) -> Self {
        KompicsRuntime {
            label: conf.label,
            throughput: conf.throughput,
        }
    }
}

impl Default for KompicsRuntime {
    fn default() -> Self {
        KompicsRuntime {
            label: default_runtime_label(),
            throughput: 50,
        }
    }
}

trait Scheduler {
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

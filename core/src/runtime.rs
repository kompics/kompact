use std::sync::Arc;
use threadpool::ThreadPool;
use std::sync::atomic::{AtomicUsize, Ordering, ATOMIC_USIZE_INIT};

use super::*;

static GLOBAL_RUNTIME_COUNT: AtomicUsize = ATOMIC_USIZE_INIT;

fn default_runtime_label() -> String {
    let runtime_count = GLOBAL_RUNTIME_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
    format!("kompics-runtime-{}", runtime_count)
}

#[derive(Clone)]
pub struct KompicsSystem {
    inner: Arc<KompicsRuntime>,
    scheduler: Scheduler,
}

impl KompicsSystem {
    pub fn new() -> KompicsSystem {
        KompicsSystem {
            inner: Arc::new(KompicsRuntime::with(default_runtime_label())),
            scheduler: Scheduler::new(),
        }
    }
    pub fn with_name(label: String) -> KompicsSystem {
        KompicsSystem {
            inner: Arc::new(KompicsRuntime::with(label)),
            scheduler: Scheduler::new(),
        }
    }

    pub fn with_threads(threads: usize) -> KompicsSystem {
        KompicsSystem {
            inner: Arc::new(KompicsRuntime::with(default_runtime_label())),
            scheduler: Scheduler::with(threads),
        }
    }

    pub fn with_throughput(throughput: usize) -> KompicsSystem {
        KompicsSystem {
            inner: Arc::new(KompicsRuntime::with_throughput(
                default_runtime_label(),
                throughput,
            )),
            scheduler: Scheduler::new(),
        }
    }

    pub fn with_throughput_threads(throughput: usize, threads: usize) -> KompicsSystem {
        KompicsSystem {
            inner: Arc::new(KompicsRuntime::with_throughput(
                default_runtime_label(),
                throughput,
            )),
            scheduler: Scheduler::with(threads),
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
}

#[derive(Clone)]
struct KompicsRuntime {
    label: String,
    throughput: usize,
}

impl KompicsRuntime {
    fn with(label: String) -> KompicsRuntime {
        KompicsRuntime {
            label,
            throughput: 1,
        }
    }

    fn with_throughput(label: String, throughput: usize) -> KompicsRuntime {
        KompicsRuntime { label, throughput }
    }

    //    fn schedule(&self, c: Arc<CoreContainer>) -> () {
    //        //println!("Scheduling component {}", c.id());
    //        self.scheduler.schedule(c);
    //    }
}

//trait Scheduler {
//    fn schedule<C: ComponentDefinition + 'static>(&self, c: Arc<Component<C>>) -> ();
//}

#[derive(Clone)]
struct Scheduler {
    pool: ThreadPool,
}

impl Scheduler {
    fn new() -> Scheduler {
        Scheduler { pool: ThreadPool::default() }
    }
    fn with(threads: usize) -> Scheduler {
        Scheduler { pool: ThreadPool::new(threads) }
    }

    fn schedule(&self, c: Arc<CoreContainer>) -> () {
        self.pool.execute(move || {
            //println!("Executing component {}", c.id());
            c.execute();
        });
    }
}

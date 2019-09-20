use crate::{
    component::{Component, ComponentDefinition, CoreContainer},
    runtime::Scheduler,
    utils,
};
use std::{
    cell::UnsafeCell,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
};

struct ThreadLocalInfo {
    reschedule: bool,
}

impl ThreadLocalInfo {
    fn reset(&mut self) {
        self.reschedule = false;
    }
    fn set(&mut self) {
        self.reschedule = true;
    }
}

thread_local!(
    static LOCAL_RESCHEDULE: UnsafeCell<Option<ThreadLocalInfo>> = UnsafeCell::new(Option::None);
);

#[derive(Clone)]
pub(crate) struct DedicatedThreadScheduler {
    handle: Arc<thread::JoinHandle<()>>,
    id: thread::ThreadId,
    stop: Arc<AtomicBool>,
    stopped: Arc<AtomicBool>,
}
impl DedicatedThreadScheduler {
    pub(crate) fn new<CD>(
    ) -> std::io::Result<(DedicatedThreadScheduler, utils::Promise<Arc<Component<CD>>>)>
    where
        CD: ComponentDefinition + 'static,
    {
        let (comp_p, comp_f) = utils::promise();
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let stopped = Arc::new(AtomicBool::new(false));
        let stopped2 = stopped.clone();
        thread::Builder::new()
            .name("dedicated-component".to_string())
            .spawn(move || DedicatedThreadScheduler::pre_run(comp_f, stop2, stopped2))
            .map(|handle| DedicatedThreadScheduler {
                handle: Arc::new(handle),
                id: thread::current().id(),
                stop,
                stopped,
            })
            .map(|scheduler| (scheduler, comp_p))
    }

    #[cfg(feature = "thread_pinning")]
    pub(crate) fn pinned<CD>(core_id: core_affinity::CoreId) -> std::io::Result<(DedicatedThreadScheduler, utils::Promise<Arc<Component<CD>>>)>
    where
        CD: ComponentDefinition + 'static,
    {
        let (comp_p, comp_f) = utils::promise();
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let stopped = Arc::new(AtomicBool::new(false));
        let stopped2 = stopped.clone();
        thread::Builder::new()
            .name("dedicated-pinned-component".to_string())
            .spawn(move || {
                core_affinity::set_for_current(core_id);
                DedicatedThreadScheduler::pre_run(comp_f, stop2, stopped2)
            })
            .map(|handle| DedicatedThreadScheduler {
                handle: Arc::new(handle),
                id: thread::current().id(),
                stop,
                stopped,
            })
            .map(|scheduler| (scheduler, comp_p))
    }

    fn pre_run<CD>(
        f: utils::Future<Arc<Component<CD>>>,
        stop: Arc<AtomicBool>,
        stopped: Arc<AtomicBool>,
    ) where
        CD: ComponentDefinition + 'static,
    {
        LOCAL_RESCHEDULE.with(|info| unsafe {
            let s = ThreadLocalInfo { reschedule: false };
            *info.get() = Some(s);
        });
        let c = f.wait(); //.expect("Should have gotten a component");
        DedicatedThreadScheduler::run(c, stop, stopped)
    }

    fn run<CD>(c: Arc<Component<CD>>, stop: Arc<AtomicBool>, stopped: Arc<AtomicBool>)
    where
        CD: ComponentDefinition + 'static,
    {
        'main: loop {
            LOCAL_RESCHEDULE.with(|info| unsafe {
                match *info.get() {
                    Some(ref mut i) => i.reset(),
                    None => unreachable!(),
                }
            });
            c.execute();
            if c.is_destroyed() || stop.load(Ordering::Relaxed) {
                break 'main;
            }
            let park = LOCAL_RESCHEDULE.with(|info| unsafe {
                match *info.get() {
                    Some(ref mut info) => !info.reschedule,
                    None => unreachable!("Did set this up there!"),
                }
            });
            if park {
                thread::park();
            }
        }
        LOCAL_RESCHEDULE.with(|info| unsafe {
            *info.get() = None;
        });
        stopped.store(true, Ordering::Relaxed);
    }

    pub(crate) fn schedule_custom(&self) -> () {
        LOCAL_RESCHEDULE.with(|info| unsafe {
            match *info.get() {
                Some(ref mut i) if self.id == thread::current().id() => {
                    i.set();
                },
                _ => {
                    self.handle.thread().unpark();
                }
            }
        });
    }
}

impl Scheduler for DedicatedThreadScheduler {
    fn schedule(&self, _c: Arc<dyn CoreContainer>) -> () {
        self.schedule_custom()
    }

    fn shutdown_async(&self) -> () {
        self.stop.store(true, Ordering::Relaxed);
    }

    fn shutdown(&self) -> Result<(), String> {
        self.stop.store(true, Ordering::Relaxed);
        loop {
            if self.stopped.load(Ordering::Relaxed) {
                return Ok(());
            } else {
                thread::yield_now();
            }
        }
    }

    fn box_clone(&self) -> Box<dyn Scheduler> {
        Box::new(self.clone()) as Box<dyn Scheduler>
    }

    fn poison(&self) -> () {
        self.stop.store(true, Ordering::Relaxed);
    }
}

use crate::{
    component::{Component, ComponentDefinition, CoreContainer, SchedulingDecision},
    runtime::Scheduler,
    utils,
};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
};

use crossbeam_utils::Backoff;

#[derive(Clone)]
pub(crate) struct DedicatedThreadScheduler {
    handle: Arc<thread::JoinHandle<()>>,
    id: thread::ThreadId,
    stop: Arc<AtomicBool>,
    stopped: Arc<AtomicBool>,
}
impl DedicatedThreadScheduler {
    pub(crate) fn new<CD>() -> std::io::Result<(
        DedicatedThreadScheduler,
        utils::KPromise<Arc<Component<CD>>>,
    )>
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
            .map(|handle| {
                let id = handle.thread().id();
                DedicatedThreadScheduler {
                    handle: Arc::new(handle),
                    id,
                    stop,
                    stopped,
                }
            })
            .map(|scheduler| (scheduler, comp_p))
    }

    #[cfg(feature = "thread_pinning")]
    pub(crate) fn pinned<CD>(
        core_id: core_affinity::CoreId,
    ) -> std::io::Result<(DedicatedThreadScheduler, utils::KPromise<Arc<Component<CD>>>)>
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
            .map(|handle| {
                let id = handle.thread().id();
                DedicatedThreadScheduler {
                    handle: Arc::new(handle),
                    id,
                    stop,
                    stopped,
                }
            })
            .map(|scheduler| (scheduler, comp_p))
    }

    fn pre_run<CD>(
        f: utils::KFuture<Arc<Component<CD>>>,
        stop: Arc<AtomicBool>,
        stopped: Arc<AtomicBool>,
    ) where
        CD: ComponentDefinition + 'static,
    {
        let c = f.wait(); //.expect("Should have gotten a component");
        DedicatedThreadScheduler::run(c, stop, stopped)
    }

    fn run<CD>(c: Arc<Component<CD>>, stop: Arc<AtomicBool>, stopped: Arc<AtomicBool>)
    where
        CD: ComponentDefinition + 'static,
    {
        let backoff = Backoff::new();
        'main: loop {
            let res = c.execute();
            if c.is_destroyed() || stop.load(Ordering::Relaxed) {
                break 'main;
            }
            let park = match res {
                SchedulingDecision::NoWork | SchedulingDecision::Blocked => true,
                SchedulingDecision::Resume | SchedulingDecision::Schedule => false,
                SchedulingDecision::AlreadyScheduled => {
                    panic!("Don't know what to do with AlreadyScheduled here")
                }
            };

            if park {
                if backoff.is_completed() {
                    thread::park();
                } else {
                    backoff.snooze();
                }
            } else {
                backoff.reset();
            }
        }
        stopped.store(true, Ordering::Relaxed);
    }

    pub(crate) fn schedule_custom(&self) -> () {
        self.handle.thread().unpark();
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

    fn spawn(&self, _future: futures::future::BoxFuture<'static, ()>) -> () {
        unimplemented!("Don't call spawn on a dedicated scheudler!");
    }
}

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

thread_local!(
    static LOCAL_RESCHEDULE: UnsafeCell<Option<bool>> = UnsafeCell::new(Option::None);
);

#[derive(Clone)]
pub(crate) struct DedicatedThreadScheduler {
    handle: Arc<thread::JoinHandle<()>>,
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
        LOCAL_RESCHEDULE.with(|flag| unsafe {
            *flag.get() = Some(false);
        });
        let c = f.wait(); //.expect("Should have gotten a component");
        DedicatedThreadScheduler::run(c, stop, stopped)
    }

    fn run<CD>(c: Arc<Component<CD>>, stop: Arc<AtomicBool>, stopped: Arc<AtomicBool>)
    where
        CD: ComponentDefinition + 'static,
    {
        'main: loop {
            LOCAL_RESCHEDULE.with(|flag| unsafe {
                *flag.get() = Some(false);
            });
            c.execute();
            if c.is_destroyed() || stop.load(Ordering::Relaxed) {
                break 'main;
            }
            let park = LOCAL_RESCHEDULE.with(|flag| unsafe {
                match *flag.get() {
                    Some(flag) => !flag,
                    None => unreachable!("Did set this up there!"),
                }
            });
            if park {
                thread::park();
            }
        }
        LOCAL_RESCHEDULE.with(|flag| unsafe {
            *flag.get() = None;
        });
        stopped.store(true, Ordering::Relaxed);
    }

    pub(crate) fn schedule_custom(&self) -> () {
        LOCAL_RESCHEDULE.with(|flag| unsafe {
            match *flag.get() {
                Some(_) => *flag.get() = Some(true),
                None => self.handle.thread().unpark(),
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

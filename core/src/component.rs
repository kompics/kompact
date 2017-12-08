use std::sync::{Arc, Weak, Mutex};
use std::cell::RefCell;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::ops::{Deref, DerefMut};
use crossbeam::sync::MsQueue;
use uuid::Uuid;

use super::*;

pub trait CoreContainer: Send + Sync {
    fn id(&self) -> &Uuid;
    fn core(&self) -> &ComponentCore;
    fn execute(&self) -> ();
}

pub struct Component<C: ComponentDefinition + Sized + 'static> {
    core: ComponentCore,
    definition: Mutex<C>,
    msg_queue: Arc<MsQueue<<ControlPort as Port>::Request>>,
}

impl<C: ComponentDefinition + Sized> Component<C> {
    pub fn new(system: KompicsSystem, definition: C) -> Component<C> {
        Component {
            core: ComponentCore::with(system),
            definition: Mutex::new(definition),
            msg_queue: Arc::new(MsQueue::new()),
        }
    }

    pub fn enqueue_control(&self, event: <ControlPort as Port>::Request) -> () {
        self.msg_queue.push(event);
        match self.core.increment_work() {
            SchedulingDecision::Schedule => {
                let system = self.core.system();
                system.schedule(self.core.component());
            }
            _ => (),// nothing
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
}

impl<C: ComponentDefinition + Sized> CoreContainer for Component<C> {
    fn id(&self) -> &Uuid {
        &self.core.id
    }
    
    fn core(&self) -> &ComponentCore {
        &self.core
    }
    fn execute(&self) -> () {
        let max_events = self.core.system.throughput();
        match self.definition().lock() {
            Ok(mut guard) => {
                let mut count: usize = 0;
                while let Some(event) = self.msg_queue.try_pop() { // ignore max_events for lifecyle events
                    //println!("Executing event: {:?}", event);
                    guard.handle(event);
                    count += 1;
                }
                if (count == 0) {
                    // only execute internal ports of no lifecycle events were pending
                    guard.execute(&self.core, max_events);
                } else {
                    match self.core.decrement_work(count as isize) { // TODO convert to checked cast
                        SchedulingDecision::Schedule => {
                            let system = self.core.system();
                            let cc = self.core.component();
                            system.schedule(cc);
                        }
                        _ => (), // ignore
                    }
                }
            }
            _ => {
                panic!("System poisoned!"); //TODO better error handling
            }
        }
    }
}

//
//pub trait Component: CoreContainer {
//    fn setup_ports(&mut self, self_component: Arc<Mutex<Self>>) -> ();
//}

pub trait ComponentDefinition: Provide<ControlPort>
where
    Self: Sized,
{
    fn setup_ports(&mut self, self_component: Arc<Component<Self>>) -> ();
    fn execute(&mut self, core: &ComponentCore, max_events: usize) -> ();
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
    work_count: AtomicIsize,
    component: RefCell<Option<Weak<CoreContainer>>>,
}

impl ComponentCore {
    pub fn with(system: KompicsSystem) -> ComponentCore {
        ComponentCore {
            id: Uuid::new_v4(),
            system,
            work_count: AtomicIsize::new(0),
            component: RefCell::default(),
        }
    }

    pub fn system(&self) -> &KompicsSystem {
        &self.system
    }

    pub(crate) fn set_component(&self, c: Arc<CoreContainer>) -> () {
        *self.component.borrow_mut() = Some(Arc::downgrade(&c));
    }

    pub fn component(&self) -> Arc<CoreContainer> {
        match *self.component.borrow() {
            Some(ref c) => {
                match c.upgrade() {
                    Some(ac) => ac,
                    None => panic!("Component already deallocated!"),
                }
            }
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
    pub fn decrement_work(&self, work_done: isize) -> SchedulingDecision {
        let oldv = self.work_count.fetch_sub(work_done, Ordering::SeqCst);
        let newv = oldv - work_done;
        if (newv > 0) {
            SchedulingDecision::Schedule
        } else {
            SchedulingDecision::NoWork
        }
    }
}

unsafe impl<C: ComponentDefinition + Sized> Send for Component<C> {}
unsafe impl<C: ComponentDefinition + Sized> Sync for Component<C> {}

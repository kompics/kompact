use std::fmt::Debug;
use std::sync::{Arc, Weak};

use super::*;

pub trait Port {
    type Indication: Sized + Send + 'static + Clone + Debug;
    type Request: Sized + Send + 'static + Clone + Debug;
}

struct CommonPortData<P: Port + 'static> {
    provide_channels: Vec<ProvidedRef<P>>,
    require_channels: Vec<RequiredRef<P>>,
}

impl<P: Port + 'static> CommonPortData<P> {
    fn new() -> CommonPortData<P> {
        CommonPortData {
            provide_channels: Vec::new(),
            require_channels: Vec::new(),
        }
    }
}

pub struct ProvidedPort<P: Port + 'static, C: ComponentDefinition + Provide<P> + 'static> {
    common: CommonPortData<P>,
    parent: Option<Weak<Component<C>>>,
    msg_queue: Arc<ConcurrentQueue<P::Request>>,
}

impl<P: Port + 'static, C: ComponentDefinition + Provide<P> + 'static> ProvidedPort<P, C> {
    pub fn new() -> ProvidedPort<P, C> {
        ProvidedPort {
            common: CommonPortData::new(),
            parent: None,
            msg_queue: Arc::new(ConcurrentQueue::new()),
        }
    }

    pub fn trigger(&mut self, event: P::Indication) -> () {
        //println!("Triggered some event!");
        for c in self.common.require_channels.iter() {
            c.enqueue(event.clone());
        }
    }
    pub fn connect(&mut self, c: RequiredRef<P>) -> () {
        self.common.require_channels.push(c);
    }

    pub fn share(&mut self) -> ProvidedRef<P> {
        match self.parent {
            Some(ref p) => {
                let core_container = p.clone() as Weak<dyn CoreContainer>;
                ProvidedRef {
                    msg_queue: Arc::downgrade(&self.msg_queue),
                    component: core_container,
                }
            }
            None => panic!("Port is not properly initialized!"),
        }
    }

    pub fn set_parent(&mut self, p: Arc<Component<C>>) -> () {
        self.parent = Some(Arc::downgrade(&p));
    }

    pub fn dequeue(&self) -> Option<P::Request> {
        self.msg_queue.pop().ok()
    }
}

pub struct RequiredPort<P: Port + 'static, C: ComponentDefinition + Require<P> + 'static> {
    common: CommonPortData<P>,
    parent: Option<Weak<Component<C>>>,
    msg_queue: Arc<ConcurrentQueue<P::Indication>>,
}

impl<P: Port + 'static, C: ComponentDefinition + Require<P> + 'static> RequiredPort<P, C> {
    pub fn new() -> RequiredPort<P, C> {
        RequiredPort {
            common: CommonPortData::new(),
            parent: None,
            msg_queue: Arc::new(ConcurrentQueue::new()),
        }
    }

    pub fn trigger(&mut self, event: P::Request) -> () {
        //println!("Triggered some event!");
        for c in self.common.provide_channels.iter() {
            c.enqueue(event.clone());
        }
    }

    pub fn connect(&mut self, c: ProvidedRef<P>) -> () {
        self.common.provide_channels.push(c);
    }

    pub fn share(&mut self) -> RequiredRef<P> {
        match self.parent {
            Some(ref p) => {
                let core_container = p.clone() as Weak<dyn CoreContainer>;
                RequiredRef {
                    msg_queue: Arc::downgrade(&self.msg_queue),
                    component: core_container,
                }
            }
            None => panic!("Port is not properly initialized!"),
        }
    }

    pub fn set_parent(&mut self, p: Arc<Component<C>>) -> () {
        self.parent = Some(Arc::downgrade(&p));
    }

    pub fn dequeue(&self) -> Option<P::Indication> {
        self.msg_queue.pop().ok()
    }

    //    pub fn execute(&self, parent: &mut Require<P>) -> () {
    //        println!("executing");
    //        while let Some(e) = self.msg_queue.try_pop() {
    //            parent.handle(e);
    //        }
    //    }
}

pub struct ProvidedRef<P: Port + 'static> {
    component: Weak<dyn CoreContainer>,
    msg_queue: Weak<ConcurrentQueue<P::Request>>,
}

impl<P: Port + 'static> Clone for ProvidedRef<P> {
    fn clone(&self) -> ProvidedRef<P> {
        ProvidedRef {
            component: self.component.clone(),
            msg_queue: self.msg_queue.clone(),
        }
    }
}

impl<P: Port + 'static> ProvidedRef<P> {
    pub(crate) fn new(
        component: Weak<dyn CoreContainer>,
        msg_queue: Weak<ConcurrentQueue<P::Request>>,
    ) -> ProvidedRef<P> {
        ProvidedRef {
            component,
            msg_queue,
        }
    }

    pub(crate) fn enqueue(&self, event: P::Request) -> () {
        match (self.msg_queue.upgrade(), self.component.upgrade()) {
            (Some(q), Some(c)) => {
                let sd = c.core().increment_work();
                q.push(event);
                match sd {
                    SchedulingDecision::Schedule => {
                        let system = c.core().system();
                        system.schedule(c.clone());
                    }
                    _ => (), // nothing
                }
            }
            (q, c) => println!(
                "Dropping event as target (queue? {:?}, component? {:?}) is unavailable: {:?}",
                q.is_some(),
                c.is_some(),
                event
            ),
        }
    }
}

pub struct RequiredRef<P: Port + 'static> {
    component: Weak<dyn CoreContainer>,
    msg_queue: Weak<ConcurrentQueue<P::Indication>>,
}

impl<P: Port + 'static> Clone for RequiredRef<P> {
    fn clone(&self) -> RequiredRef<P> {
        RequiredRef {
            component: self.component.clone(),
            msg_queue: self.msg_queue.clone(),
        }
    }
}

impl<P: Port + 'static> RequiredRef<P> {
    pub(crate) fn enqueue(&self, event: P::Indication) -> () {
        match (self.msg_queue.upgrade(), self.component.upgrade()) {
            (Some(q), Some(c)) => {
                let sd = c.core().increment_work();
                q.push(event);
                match sd {
                    SchedulingDecision::Schedule => {
                        let system = c.core().system();
                        system.schedule(c.clone());
                    }
                    _ => (), // nothing
                }
            }
            (q, c) => println!(
                "Dropping event as target (queue? {:?}, component? {:?}) is unavailable: {:?}",
                q.is_some(),
                c.is_some(),
                event
            ),
        }
    }
}

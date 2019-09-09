use super::prelude::*;
use crate::utils::{Fulfillable, Promise};

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

pub(crate) struct SupervisionPort;

impl Port for SupervisionPort {
    type Indication = ();
    type Request = SupervisorMsg;
}

#[derive(Debug, Clone)]
pub(crate) enum ListenEvent {
    Started(Uuid),
    Stopped(Uuid),
    Destroyed(Uuid),
}

impl ListenEvent {
    fn id(&self) -> &Uuid {
        match self {
            ListenEvent::Started(ref id) => id,
            ListenEvent::Stopped(ref id) => id,
            ListenEvent::Destroyed(ref id) => id,
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(crate) enum SupervisorMsg {
    Started(Arc<dyn CoreContainer>),
    Stopped(Uuid),
    Killed(Uuid),
    Faulty(Uuid), // TODO implement fault handling!
    Listen(Arc<Mutex<Promise<()>>>, ListenEvent),
    Shutdown(Promise<()>),
}

#[derive(ComponentDefinition, Actor)]
pub(crate) struct ComponentSupervisor {
    ctx: ComponentContext<ComponentSupervisor>,
    pub(crate) supervision: ProvidedPort<SupervisionPort, ComponentSupervisor>,
    children: HashMap<Uuid, Arc<dyn CoreContainer>>,
    listeners: HashMap<Uuid, Vec<(ListenEvent, Promise<()>)>>,
    shutdown: Option<Promise<()>>,
}

impl ComponentSupervisor {
    pub(crate) fn new() -> ComponentSupervisor {
        ComponentSupervisor {
            ctx: ComponentContext::new(),
            supervision: ProvidedPort::new(),
            children: HashMap::new(),
            listeners: HashMap::new(),
            shutdown: None,
        }
    }

    fn notify_listeners<S>(&mut self, id: &Uuid, selector: S)
    where
        S: Fn(&ListenEvent) -> bool,
    {
        if let Some(l) = self.listeners.get_mut(id) {
            let mut selected = l
                .drain_filter(|entry| selector(&entry.0))
                .collect::<Vec<_>>();
            for (_, p) in selected.drain(..) {
                p.fulfill(()).unwrap_or_else(|e| {
                    error!(self.ctx.log(), "Could not notify listeners: {:?}", e)
                });
            }
        }
    }
    fn drop_listeners(&mut self, id: &Uuid) {
        self.listeners.remove(id);
    }
}

impl Provide<ControlPort> for ComponentSupervisor {
    fn handle(&mut self, event: ControlEvent) -> () {
        match event {
            ControlEvent::Start => {
                debug!(self.ctx.log(), "Started.");
            }
            ControlEvent::Stop => {
                error!(self.ctx.log(), "Do not stop the supervisor!");
                panic!("Invalid supervisor handling!");
            }
            ControlEvent::Kill => {
                error!(
                    self.ctx.log(),
                    "Use SupervisorMsg::Shutdown to kill the supervisor!"
                );
                panic!("Invalid supervisor handling!");
            }
        }
    }
}

impl Provide<SupervisionPort> for ComponentSupervisor {
    fn handle(&mut self, event: SupervisorMsg) -> () {
        match event {
            SupervisorMsg::Started(c) => {
                let id = c.id().clone();
                let ctrl = c.control_port();
                self.children.insert(id, c);
                debug!(self.ctx.log(), "Component({}) was started.", id);
                self.notify_listeners(&id, |l| {
                    if let ListenEvent::Started(_) = l {
                        true
                    } else {
                        false
                    }
                });
                if self.shutdown.is_some() {
                    warn!(
                        self.ctx.log(),
                        "Child {} just started during shutdown. Killing it immediately!", id
                    );
                    ctrl.enqueue(ControlEvent::Kill);
                }
            }
            SupervisorMsg::Stopped(id) => {
                debug!(self.ctx.log(), "Component({}) was stopped.", id);
                self.notify_listeners(&id, |l| {
                    if let ListenEvent::Stopped(_) = l {
                        true
                    } else {
                        false
                    }
                });
            }
            SupervisorMsg::Killed(id) => {
                match self.children.remove(&id) {
                    Some(carc) => {
                        let count = Arc::strong_count(&carc);
                        drop(carc);
                        if (count == 1) {
                            debug!(
                                self.ctx.log(),
                                "Component({}) was killed and deallocated.", id
                            ); // probably^^
                        } else {
                            warn!(self.ctx.log(), "Component({}) was killed but there are still outstanding references preventing deallocation.", id);
                        }
                        self.notify_listeners(&id, |l| {
                            if let ListenEvent::Destroyed(_) = l {
                                true
                            } else {
                                false
                            }
                        });
                        self.drop_listeners(&id);
                    }
                    None => warn!(self.ctx.log(), "An untracked Component({}) was killed.", id),
                }
                if self.shutdown.is_some() {
                    if self.children.is_empty() {
                        debug!(self.ctx.log(), "Last child of the Supervisor is dead!");
                        let promise = self.shutdown.take().unwrap();
                        promise
                            .fulfill(())
                            .expect("Could not fulfill shutdown promise!");
                        self.listeners.clear(); // we won't be fulfilling these anyway
                    } else {
                        trace!(
                            self.ctx.log(),
                            "Supervisor is still dying with {} children outstanding...",
                            self.children.len()
                        );
                    }
                }
            }
            SupervisorMsg::Faulty(id) => {
                warn!(
                    self.ctx.log(),
                    "Component({}) has been marked as faulty.", id
                );
                self.drop_listeners(&id); // will never be fulfilled
                match self.children.remove(&id) {
                    Some(carc) => drop(carc),
                    None => warn!(self.ctx.log(), "Component({}) faulted during start!.", id),
                }
            }
            SupervisorMsg::Listen(amp, event) => match Arc::try_unwrap(amp) {
                Ok(mp) => {
                    let p = mp
                        .into_inner()
                        .expect("Someone broke the promise mutex -.-");
                    trace!(self.ctx.log(), "Subscribing listener for {}.", event.id());
                    let l = self
                        .listeners
                        .entry(event.id().clone())
                        .or_insert(Vec::new());
                    l.push((event, p));
                }
                Err(_) => error!(
                    self.ctx.log(),
                    "Can't unwrap listen event on {}. Dropping.",
                    event.id()
                ),
            },
            SupervisorMsg::Shutdown(promise) => {
                debug!(self.ctx.log(), "Supervisor got shutdown request.");
                if self.children.is_empty() {
                    trace!(self.ctx.log(), "Supervisor has no children!");
                    promise
                        .fulfill(())
                        .expect("Could not fulfill shutdown promise!");
                    self.listeners.clear(); // we won't be fulfilling these anyway
                } else {
                    trace!(self.ctx.log(), "Killing {} children.", self.children.len());
                    self.shutdown = Some(promise);
                    for child in self.children.values() {
                        child.control_port().enqueue(ControlEvent::Kill);
                    }
                }
            }
        }
    }
}

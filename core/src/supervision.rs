use super::prelude::*;
use crate::{
    utils::{Fulfillable, KPromise},
    ControlEvent,
};

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
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
    fn id(&self) -> Uuid {
        match self {
            ListenEvent::Started(id) => *id,
            ListenEvent::Stopped(id) => *id,
            ListenEvent::Destroyed(id) => *id,
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(crate) enum SupervisorMsg {
    Started(Arc<dyn CoreContainer>),
    Stopped(Uuid),
    Killed(Uuid),
    Faulty(Uuid),
    Listen(Arc<Mutex<KPromise<()>>>, ListenEvent),
    Shutdown(Arc<Mutex<KPromise<()>>>),
}

#[derive(ComponentDefinition, Actor)]
pub(crate) struct ComponentSupervisor {
    ctx: ComponentContext<ComponentSupervisor>,
    pub(crate) supervision: ProvidedPort<SupervisionPort>,
    children: HashMap<Uuid, Arc<dyn CoreContainer>>,
    listeners: HashMap<Uuid, Vec<(ListenEvent, KPromise<()>)>>,
    shutdown: Option<KPromise<()>>,
}

impl ComponentSupervisor {
    pub(crate) fn new() -> ComponentSupervisor {
        ComponentSupervisor {
            ctx: ComponentContext::uninitialised(),
            supervision: ProvidedPort::uninitialised(),
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
            let (mut selected, mut unselected) = l.drain(..).partition(|entry| selector(&entry.0));
            std::mem::swap(l, &mut unselected);
            // requires #![feature(drain_filter)]
            // let mut selected = l
            //     .drain_filter(|entry| selector(&entry.0))
            //     .collect::<Vec<_>>();
            for (_, p) in selected.drain(..) {
                p.fulfil(()).unwrap_or_else(|e| {
                    error!(self.ctx.log(), "Could not notify listeners: {:?}", e)
                });
            }
        }
    }

    fn drop_listeners(&mut self, id: &Uuid) {
        self.listeners.remove(id);
    }

    fn shutdown_if_no_more_children(&mut self) {
        if self.shutdown.is_some() {
            if self.children.is_empty() {
                debug!(
                    self.ctx.log(),
                    "Last child of the Supervisor is dead or faulty!"
                );
                let promise = self.shutdown.take().unwrap();
                promise
                    .fulfil(())
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
}

impl ComponentLifecycle for ComponentSupervisor {
    fn on_start(&mut self) -> Handled {
        debug!(self.ctx.log(), "Started supervisor.");
        Handled::Ok
    }

    fn on_stop(&mut self) -> Handled {
        error!(self.ctx.log(), "Do not stop the supervisor!");
        panic!("Invalid supervisor handling!");
    }

    fn on_kill(&mut self) -> Handled {
        error!(
            self.ctx.log(),
            "Use SupervisorMsg::Shutdown to kill the supervisor!"
        );
        panic!("Invalid supervisor handling!");
    }
}

impl Provide<SupervisionPort> for ComponentSupervisor {
    fn handle(&mut self, event: SupervisorMsg) -> Handled {
        match event {
            SupervisorMsg::Started(c) => {
                let id = c.id();
                self.children.insert(id, c.clone());
                debug!(self.ctx.log(), "Component({}) was started.", id);
                self.notify_listeners(&id, |l| matches!(l, ListenEvent::Started(_)));
                if self.shutdown.is_some() {
                    warn!(
                        self.ctx.log(),
                        "Child {} just started during shutdown. Killing it immediately!", id
                    );
                    c.enqueue_control(ControlEvent::Kill);
                }
            }
            SupervisorMsg::Stopped(id) => {
                debug!(self.ctx.log(), "Component({}) was stopped.", id);
                self.notify_listeners(&id, |l| matches!(l, ListenEvent::Stopped(_)));
            }
            SupervisorMsg::Killed(id) => {
                match self.children.remove(&id) {
                    Some(carc) => {
                        let count = Arc::strong_count(&carc);
                        drop(carc);
                        if count == 1 {
                            trace!(
                                self.ctx.log(),
                                "Component({}) was killed and deallocated.",
                                id
                            ); // probably^^
                        } else {
                            debug!(self.ctx.log(), "Component({}) was killed but there are still outstanding references preventing deallocation.", id);
                        }
                        self.notify_listeners(&id, |l| matches!(l, ListenEvent::Destroyed(_)));
                        self.drop_listeners(&id);
                    }
                    None => warn!(self.ctx.log(), "An untracked Component({}) was killed.", id),
                }
                self.shutdown_if_no_more_children()
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
                self.shutdown_if_no_more_children()
            }
            SupervisorMsg::Listen(amp, event) => match Arc::try_unwrap(amp) {
                Ok(mp) => {
                    let p = mp
                        .into_inner()
                        .expect("Someone broke the promise mutex -.-");
                    trace!(self.ctx.log(), "Subscribing listener for {}.", event.id());
                    let l = self.listeners.entry(event.id()).or_insert_with(Vec::new);
                    l.push((event, p));
                }
                Err(_) => error!(
                    self.ctx.log(),
                    "Can't unwrap listen event on {}. Dropping.",
                    event.id()
                ),
            },
            SupervisorMsg::Shutdown(amp) => match Arc::try_unwrap(amp) {
                Ok(mp) => {
                    let promise = mp
                        .into_inner()
                        .expect("Someone broke the promise mutex -.-");
                    debug!(self.ctx.log(), "Supervisor got shutdown request.");
                    if self.children.is_empty() {
                        trace!(self.ctx.log(), "Supervisor has no children!");
                        promise
                            .fulfil(())
                            .expect("Could not fulfill shutdown promise!");
                        self.listeners.clear(); // we won't be fulfilling these anyway
                    } else {
                        trace!(self.ctx.log(), "Killing {} children.", self.children.len());
                        self.shutdown = Some(promise);
                        for child in self.children.values() {
                            child.enqueue_control(ControlEvent::Kill);
                        }
                    }
                }
                Err(_) => error!(
                    self.ctx.log(),
                    "Can't unwrap listen event on Shutdown. Dropping."
                ),
            },
        }
        Handled::Ok
    }
}

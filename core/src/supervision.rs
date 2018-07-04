use super::*;

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

#[derive(Debug, Clone)]
pub(crate) enum SupervisorMsg {
    Started(Arc<CoreContainer>),
    Stopped(Uuid),
    Killed(Uuid),
    Faulty(Uuid),
    Listen(Arc<Mutex<Promise<()>>>, ListenEvent),
}

#[derive(ComponentDefinition, Actor)]
pub(crate) struct ComponentSupervisor {
    ctx: ComponentContext<ComponentSupervisor>,
    pub(crate) supervision: ProvidedPort<SupervisionPort, ComponentSupervisor>,
    children: HashMap<Uuid, Arc<CoreContainer>>,
    listeners: HashMap<Uuid, Vec<(ListenEvent, Promise<()>)>>,
}

impl ComponentSupervisor {
    pub(crate) fn new() -> ComponentSupervisor {
        ComponentSupervisor {
            ctx: ComponentContext::new(),
            supervision: ProvidedPort::new(),
            children: HashMap::new(),
            listeners: HashMap::new(),
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
                p.fulfill(());
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
                println!("[Supervisor] Starting.");
            }
            ControlEvent::Stop => {
                println!("[Supervisor] Stopping.");
            }
            ControlEvent::Kill => {
                println!("[Supervisor] Killed.");
            }
        }
    }
}

impl Provide<SupervisionPort> for ComponentSupervisor {
    fn handle(&mut self, event: SupervisorMsg) -> () {
        match event {
            SupervisorMsg::Started(c) => {
                let id = c.id().clone();
                self.children.insert(id, c);
                println!("[Supervisor] Component({}) was started.", id);
                self.notify_listeners(&id, |l| {
                    if let ListenEvent::Started(_) = l {
                        true
                    } else {
                        false
                    }
                });
            }
            SupervisorMsg::Stopped(id) => {
                println!("[Supervisor] Component({}) was stopped.", id);
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
                            println!("[Supervisor] Component({}) was killed and deallocated.", id); // probably^^
                        } else {
                            println!("[Supervisor] Component({}) was killed but there are still outstanding references preventing deallocation.", id);
                        }
                        self.notify_listeners(&id, |l| {
                            if let ListenEvent::Destroyed(_) = l {
                                true
                            } else {
                                false
                            }
                        });
                        self.drop_listeners(&id);
                        // match Arc::try_unwrap(carc) {
                        // 	Ok(_) =>
                        // 	Err(_) => println!("Component({}) was killed but there are still outstanding references preventing deallocation.", id),
                        // }
                    }
                    None => println!("[Supervisor] An untracked Component({}) was killed.", id),
                }
            }
            SupervisorMsg::Faulty(id) => {
                println!("[Supervisor] Component({}) has been marked as faulty.", id);
                // TODO recovery
            }
            SupervisorMsg::Listen(amp, event) => match Arc::try_unwrap(amp) {
                Ok(mp) => {
                    let p = mp
                        .into_inner()
                        .expect("Someone broke the promise mutex -.-");
                    println!("[Supervisor] Subscribing listener for {}.", event.id());
                    let l = self
                        .listeners
                        .entry(event.id().clone())
                        .or_insert(Vec::new());
                    l.push((event, p));
                }
                Err(_) => eprintln!(
                    "[Supervisor] Can't unwrap listen event on {}. Dropping.",
                    event.id()
                ),
            },
        }
    }
}

use super::prelude::*;
use crate::{
    component::ContextSystemHandle,
    utils::{Completable, KPromise},
    ControlEvent,
    KompactLogger,
};

use std::{
    collections::HashMap,
    fmt,
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

/// Information about the fault that occurred
pub struct FaultContext {
    /// The id of the component that faulted
    pub component_id: Uuid,
    /// The concrete error produced by [catch_unwind](std::panic::catch_unwind)
    pub fault: Box<dyn Any + Send>,
}
impl FaultContext {
    pub(crate) fn new(component_id: Uuid, fault: Box<dyn Any + Send>) -> Self {
        FaultContext {
            component_id,
            fault,
        }
    }

    /// Produce a [Recoverhandler](RecoveryHandler) with `f` describing
    /// the actions to take in order to recover from this fault
    ///
    /// The action described by this object will be executed on the Supervisor.
    /// This means you *cannot* block on the result of any notify futures,
    /// without *deadlocking* your system!
    ///
    /// If you need to perform a complicated setup sequence, consider starting
    /// a temporary component to drive the futures involved, instead of handling
    /// everything in the recovery handler.
    pub fn recover_with<F>(self, f: F) -> RecoveryHandler
    where
        F: FnOnce(Self, ContextSystemHandle, &KompactLogger) + Send + 'static,
    {
        let boxed = Box::new(f);
        RecoveryHandler {
            ctx: self,
            action: boxed,
        }
    }

    /// Simply ignore the fault and don't recover at all
    pub fn ignore(self) -> RecoveryHandler {
        self.recover_with(move |ctx, _, logger| {
            warn!(
                logger,
                "Ignoring fault of component id={}", ctx.component_id
            );
        })
    }

    /// Create and start the [Default](Default) instance of `C`
    pub fn restart_default<C>(self) -> RecoveryHandler
    where
        C: ComponentDefinition + Default,
    {
        self.recover_with(move |ctx, system, logger| {
            info!(
                logger,
                "Restarting a {} to replace instance with id={}",
                C::type_name(),
                ctx.component_id
            );
            let cd = system.create(C::default);
            system.start(&cd);
        })
    }
}
impl fmt::Debug for FaultContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let error_msg: String = if let Some(error_msg) = self.fault.downcast_ref::<&str>() {
            error_msg.to_string()
        } else if let Some(error_msg) = self.fault.downcast_ref::<String>() {
            error_msg.clone()
        } else {
            format!(
                "Component panicked with a non-string message with type id={:?}",
                self.fault.type_id()
            )
        };
        f.debug_struct("RecoveryHandler")
            .field("component_id", &self.component_id)
            .field("action", &error_msg)
            .finish()
    }
}

/// Instructions on how to recover from a component fault
///
/// The action described by this object will be executed on the Supervisor.
/// This means you *cannot* block on the result of any notify futures,
/// without *deadlocking* your system!
///
/// If you need to perform a complicated setup sequence, consider starting
/// a temporary component to drive the futures involved, instead of handling
/// everything in the recovery handler.
///
/// An instance of this can be produced via [FaultContext::recover_with](crate::prelude::FaultContext::recover_with)
/// or other convenience methods on [FaultContext](crate::prelude::FaultContext).
pub struct RecoveryHandler {
    /// The context of the fault that occurred
    ctx: FaultContext,
    /// The actions to take in response to the fault
    #[allow(clippy::type_complexity)]
    action: Box<dyn FnOnce(FaultContext, ContextSystemHandle, &KompactLogger) + Send>,
}
impl RecoveryHandler {
    fn recover(self, system: ContextSystemHandle, logger: &KompactLogger) -> () {
        (self.action)(self.ctx, system, logger)
    }
}
impl fmt::Debug for RecoveryHandler {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RecoveryHandler")
            .field("ctx", &self.ctx)
            .field("action", &"<func>")
            .finish()
    }
}
impl Clone for RecoveryHandler {
    fn clone(&self) -> Self {
        // This is a bit silly but port types require Clone,
        // even if there's only a single supervisor.
        panic!("Don't clone recovery handlers!");
    }
}

#[derive(Debug, Clone)]
pub(crate) enum SupervisorMsg {
    Started(Arc<dyn CoreContainer>),
    Stopped(Uuid),
    Killed(Uuid),
    Faulty(RecoveryHandler),
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
            for (_, p) in selected.drain(..) {
                p.complete().unwrap_or_else(|e| {
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
                    .complete()
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
            SupervisorMsg::Faulty(recover_handler) => {
                let id = recover_handler.ctx.component_id;
                warn!(
                    self.ctx.log(),
                    "Component({}) has been marked as faulty.", id
                );
                self.drop_listeners(&id); // will never be fulfilled
                match self.children.remove(&id) {
                    Some(carc) => drop(carc),
                    None => warn!(self.ctx.log(), "Component({}) faulted during start!.", id),
                }
                if self.shutdown.is_none() {
                    recover_handler.recover(self.ctx.context_system(), self.ctx.log());
                } else {
                    warn!(
                        self.log(),
                        "Not running recovery handler due to ongoing shutdown."
                    );
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
                            .complete()
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

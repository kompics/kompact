use super::*;
use crate::messaging::{DispatchEnvelope, NetMessage};
use std::sync::Arc;

pub(crate) struct DefaultComponents {
    deadletter_box: Arc<Component<DeadletterBox>>,
    dispatcher: Arc<Component<LocalDispatcher>>,
}

impl DefaultComponents {
    pub(crate) fn new(
        system: &KompactSystem,
        dead_prom: Promise<()>,
        disp_prom: Promise<()>,
    ) -> DefaultComponents {
        let dbc = system.create_unsupervised(|| DeadletterBox::new(dead_prom));
        let ldc = system.create_unsupervised(|| LocalDispatcher::new(disp_prom));
        DefaultComponents {
            deadletter_box: dbc,
            dispatcher: ldc,
        }
    }
}

impl SystemComponents for DefaultComponents {
    // fn timer_ref(&self) -> timer::TimerRef {
    //     self.timer.timer_ref()
    // }
    fn deadletter_ref(&self) -> ActorRef<Never> {
        self.deadletter_box.actor_ref()
    }

    fn dispatcher_ref(&self) -> DispatcherRef {
        self.dispatcher
            .actor_ref()
            .hold()
            .expect("Dispatcher should not be deallocated!")
    }

    fn system_path(&self) -> SystemPath {
        self.dispatcher.on_definition(|cd| cd.system_path())
    }

    fn start(&self, system: &KompactSystem) -> () {
        system.start(&self.deadletter_box);
        system.start(&self.dispatcher);
    }

    fn stop(&self, system: &KompactSystem) -> () {
        system.kill(self.dispatcher.clone());
        system.kill(self.deadletter_box.clone());
        self.dispatcher.wait_ended();
        self.deadletter_box.wait_ended();
    }
}

pub(crate) struct DefaultTimer {
    inner: timer::TimerWithThread,
}

impl DefaultTimer {
    pub(crate) fn new() -> DefaultTimer {
        DefaultTimer {
            inner: timer::TimerWithThread::new().unwrap(),
        }
    }

    pub(crate) fn new_timer_component() -> Box<dyn TimerComponent> {
        let t = DefaultTimer::new();
        let bt = Box::new(t) as Box<dyn TimerComponent>;
        bt
    }
}

impl TimerRefFactory for DefaultTimer {
    fn timer_ref(&self) -> timer::TimerRef {
        self.inner.timer_ref()
    }
}
impl TimerComponent for DefaultTimer {
    fn shutdown(&self) -> Result<(), String> {
        self.inner
            .shutdown_async()
            .map_err(|e| format!("Error during timer shutdown: {:?}", e))
    }
}

/// A wrapper for custom system components
///
/// This struct already has [SystemComponents][SystemComponents] implemented for it,
/// saving you the work.
pub struct CustomComponents<B, C>
where
    B: ComponentDefinition + ActorRaw<Message = Never> + Sized + 'static,
    C: ComponentDefinition + ActorRaw<Message = DispatchEnvelope> + Sized + 'static + Dispatcher,
{
    pub(crate) deadletter_box: Arc<Component<B>>,
    pub(crate) dispatcher: Arc<Component<C>>,
}

impl<B, C> SystemComponents for CustomComponents<B, C>
where
    B: ComponentDefinition + ActorRaw<Message = Never> + Sized + 'static,
    C: ComponentDefinition + ActorRaw<Message = DispatchEnvelope> + Sized + 'static + Dispatcher,
{
    fn deadletter_ref(&self) -> ActorRef<Never> {
        self.deadletter_box.actor_ref()
    }

    fn dispatcher_ref(&self) -> DispatcherRef {
        self.dispatcher
            .actor_ref()
            .hold()
            .expect("Dispatcher should not be deallocated!")
    }

    fn system_path(&self) -> SystemPath {
        self.dispatcher.on_definition(|cd| cd.system_path())
    }

    fn start(&self, system: &KompactSystem) -> () {
        system.start(&self.deadletter_box);
        system.start(&self.dispatcher);
    }

    fn stop(&self, system: &KompactSystem) -> () {
        system.kill(self.dispatcher.clone());
        system.kill(self.deadletter_box.clone());
        self.dispatcher.wait_ended();
        self.deadletter_box.wait_ended();
    }
}

/// The default deadletter box
///
/// Simply logs every received message at the `info` level
/// and then discards it.
#[derive(ComponentDefinition)]
pub struct DeadletterBox {
    ctx: ComponentContext<DeadletterBox>,
    notify_ready: Option<Promise<()>>,
}

impl DeadletterBox {
    /// Creates a new deadletter box
    ///
    /// The `notify_ready` promise will be fulfilled, when the component
    /// received a [Start](ControlEvent::Start) event.
    pub fn new(notify_ready: Promise<()>) -> DeadletterBox {
        DeadletterBox {
            ctx: ComponentContext::new(),
            notify_ready: Some(notify_ready),
        }
    }
}

impl Actor for DeadletterBox {
    type Message = Never;

    /// Handles local messages.
    fn receive_local(&mut self, _msg: Self::Message) -> () {
        unimplemented!(); // this can't actually happen
    }

    /// Handles (serialised or reflected) messages from the network.
    fn receive_network(&mut self, msg: NetMessage) -> () {
        info!(
            self.ctx.log(),
            "DeadletterBox received network message {:?}", msg,
        );
    }
}

impl Provide<ControlPort> for DeadletterBox {
    fn handle(&mut self, event: ControlEvent) -> () {
        match event {
            ControlEvent::Start => {
                debug!(self.ctx.log(), "Starting DeadletterBox");
                match self.notify_ready.take() {
                    Some(promise) => promise.fulfil(()).unwrap_or_else(|_| ()),
                    None => (),
                }
            }
            _ => (), // ignore
        }
    }
}

/// The default non-networked dispatcher
///
/// Logs network messages at the `info` level and local messages at the `warn` level,
/// then discards either.
#[derive(ComponentDefinition)]
pub struct LocalDispatcher {
    ctx: ComponentContext<LocalDispatcher>,
    notify_ready: Option<Promise<()>>,
}

impl LocalDispatcher {
    /// Creates a new local dispatcher
    ///
    /// The `notify_ready` promise will be fulfilled, when the component
    /// received a [Start](ControlEvent::Start) event.
    pub fn new(notify_ready: Promise<()>) -> LocalDispatcher {
        LocalDispatcher {
            ctx: ComponentContext::new(),
            notify_ready: Some(notify_ready),
        }
    }
}

impl Actor for LocalDispatcher {
    type Message = DispatchEnvelope;

    fn receive_local(&mut self, msg: Self::Message) -> () {
        warn!(
            self.ctx.log(),
            "LocalDispatcher received {:?}, but doesn't know what to do with it (hint: implement dispatching ;)",
            msg,
        );
    }

    fn receive_network(&mut self, msg: NetMessage) -> () {
        info!(
            self.ctx.log(),
            "LocalDispatcher received network message {:?}", msg,
        );
    }
}

impl Dispatcher for LocalDispatcher {
    fn system_path(&mut self) -> SystemPath {
        SystemPath::new(Transport::LOCAL, "127.0.0.1".parse().unwrap(), 0)
    }
}

impl Provide<ControlPort> for LocalDispatcher {
    fn handle(&mut self, event: ControlEvent) -> () {
        match event {
            ControlEvent::Start => {
                debug!(self.ctx.log(), "Starting LocalDispatcher");
                match self.notify_ready.take() {
                    Some(promise) => promise.fulfil(()).unwrap_or_else(|_| ()),
                    None => (),
                }
            }
            _ => (), // ignore
        }
    }
}

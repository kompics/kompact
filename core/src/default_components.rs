use super::*;
use crate::{
    dispatch::NetworkStatusPort,
    messaging::{DispatchEnvelope, NetMessage},
    timer::timer_manager::TimerRefFactory,
};
use std::sync::Arc;

pub(crate) struct DefaultComponents {
    deadletter_box: Arc<Component<DeadletterBox>>,
    dispatcher: Arc<Component<LocalDispatcher>>,
}

impl DefaultComponents {
    pub(crate) fn new(
        system: &KompactSystem,
        dead_prom: KPromise<()>,
        disp_prom: KPromise<()>,
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

    fn kill(&self, system: &KompactSystem) -> () {
        // default_components has no graceful shutdown sequence to bypass
        self.stop(system);
    }

    fn connect_network_status_port(&self, _required: &mut RequiredPort<NetworkStatusPort>) -> () {
        unimplemented!("System must have a NetworkDispatcher to use the NetworkStatusPort")
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
        Box::new(t) as Box<dyn TimerComponent>
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
        // Gracefully stop the dispatcher/Network layer.
        system.stop(&self.dispatcher.clone());
        self.kill(system);
    }

    fn kill(&self, system: &KompactSystem) -> () {
        system.kill(self.dispatcher.clone());
        system.kill(self.deadletter_box.clone());
        self.dispatcher.wait_ended();
        self.deadletter_box.wait_ended();
    }

    fn connect_network_status_port(&self, required: &mut RequiredPort<NetworkStatusPort>) -> () {
        self.dispatcher.on_definition(|d| {
            utils::biconnect_ports(d.network_status_port(), required);
        })
    }
}

/// The default deadletter box
///
/// Simply logs every received message at the `info` level
/// and then discards it.
#[derive(ComponentDefinition)]
pub struct DeadletterBox {
    ctx: ComponentContext<DeadletterBox>,
    notify_ready: Option<KPromise<()>>,
}

impl DeadletterBox {
    /// Creates a new deadletter box
    ///
    /// The `notify_ready` promise will be fulfilled, when the component
    /// received a `Start` event.
    pub fn new(notify_ready: KPromise<()>) -> DeadletterBox {
        DeadletterBox {
            ctx: ComponentContext::uninitialised(),
            notify_ready: Some(notify_ready),
        }
    }
}

impl Actor for DeadletterBox {
    type Message = Never;

    /// Handles local messages.
    fn receive_local(&mut self, _msg: Self::Message) -> Handled {
        unimplemented!(); // this can't actually happen
    }

    /// Handles (serialised or reflected) messages from the network.
    fn receive_network(&mut self, msg: NetMessage) -> Handled {
        info!(
            self.ctx.log(),
            "DeadletterBox received network message {:?}", msg,
        );
        Handled::Ok
    }
}

impl ComponentLifecycle for DeadletterBox {
    fn on_start(&mut self) -> Handled {
        debug!(self.ctx.log(), "Starting DeadletterBox");
        if let Some(promise) = self.notify_ready.take() {
            promise.fulfil(()).unwrap_or(())
        }
        Handled::Ok
    }
}

/// The default non-networked dispatcher
///
/// Logs network messages at the `info` level and local messages at the `warn` level,
/// then discards either.
#[derive(ComponentDefinition)]
pub struct LocalDispatcher {
    ctx: ComponentContext<LocalDispatcher>,
    notify_ready: Option<KPromise<()>>,
}

impl LocalDispatcher {
    /// Creates a new local dispatcher
    ///
    /// The `notify_ready` promise will be fulfilled, when the component
    /// received a `Start` event.
    pub fn new(notify_ready: KPromise<()>) -> LocalDispatcher {
        LocalDispatcher {
            ctx: ComponentContext::uninitialised(),
            notify_ready: Some(notify_ready),
        }
    }
}

impl Actor for LocalDispatcher {
    type Message = DispatchEnvelope;

    fn receive_local(&mut self, msg: Self::Message) -> Handled {
        use crate::messaging::{RegistrationEnvelope, RegistrationError, RegistrationPromise};
        warn!(
            self.ctx.log(),
            "LocalDispatcher received {:?}, but doesn't know what to do with it (hint: implement dispatching ;)",
            msg,
        );
        if let DispatchEnvelope::Registration(RegistrationEnvelope { promise, .. }) = msg {
            if let RegistrationPromise::Fulfil(p) = promise {
                p.fulfil(Err(RegistrationError::Unsupported))
                    .unwrap_or_else(|e| {
                        error!(self.ctx.log(), "Could not notify listeners: {:?}", e)
                    });
            }
        } else {
            error!(self.ctx.log(), "Ignoring message {:?}.", msg);
        }
        Handled::Ok
    }

    fn receive_network(&mut self, msg: NetMessage) -> Handled {
        info!(
            self.ctx.log(),
            "LocalDispatcher received network message {:?}", msg,
        );
        Handled::Ok
    }
}

impl Dispatcher for LocalDispatcher {
    fn system_path(&mut self) -> SystemPath {
        SystemPath::new(Transport::Local, "127.0.0.1".parse().unwrap(), 0)
    }

    fn network_status_port(&mut self) -> &mut ProvidedPort<NetworkStatusPort> {
        unimplemented!("The LocalDispatcher does not provide a NetworkStatusPort");
    }
}

impl ComponentLifecycle for LocalDispatcher {
    fn on_start(&mut self) -> Handled {
        debug!(self.ctx.log(), "Starting LocalDispatcher");
        if let Some(promise) = self.notify_ready.take() {
            promise.fulfil(()).unwrap_or(())
        }
        Handled::Ok
    }
}

use super::*;
#[cfg(feature = "distributed")]
use crate::messaging::NetMessage;
use crate::{
    dispatch::lookup::{ActorLookup, ActorStore, LookupResult},
    messaging::{
        ActorRegistration,
        DispatchData,
        DispatchEnvelope,
        MsgEnvelope,
        PathResolvable,
        PolicyRegistration,
        RegistrationEnvelope,
        RegistrationError,
        RegistrationEvent,
        RegistrationPromise,
    },
    timer::timer_manager::TimerRefFactory,
};
use std::{any::Any, sync::Arc};

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

    fn with_dispatcher_definition_dyn(&self, f: &mut dyn FnMut(&mut dyn Any)) -> () {
        self.dispatcher.on_definition(|dispatcher| f(dispatcher));
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

struct ManualTimerComponent {
    inner: timer::ManualTimer,
}

impl ManualTimerComponent {
    fn new(inner: timer::ManualTimer) -> Self {
        Self { inner }
    }
}

impl TimerRefFactory for ManualTimerComponent {
    fn timer_ref(&self) -> timer::TimerRef {
        self.inner.timer_ref()
    }
}

impl TimerComponent for ManualTimerComponent {
    fn shutdown(&self) -> Result<(), String> {
        self.inner.stop();
        Ok(())
    }
}

/// Replaces the default threaded timer with a manually-driven timer and
/// returns the control handle for tests.
///
/// The returned timer can be advanced deterministically with
/// [`timer::ManualTimer::advance_by`] or [`timer::ManualTimer::advance_to_next`].
pub fn install_manual_timer(config: &mut KompactConfig) -> timer::ManualTimer {
    let timer = timer::ManualTimer::new();
    let builder_timer = timer.clone();
    config.timer::<ManualTimerComponent, _>(move || {
        Box::new(ManualTimerComponent::new(builder_timer.clone()))
    });
    timer
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

    fn with_dispatcher_definition_dyn(&self, f: &mut dyn FnMut(&mut dyn Any)) -> () {
        self.dispatcher.on_definition(|dispatcher| f(dispatcher));
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

    #[cfg(feature = "distributed")]
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
            promise.complete().unwrap_or(())
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
    lookup: ActorStore,
    system_path: SystemPath,
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
            lookup: ActorStore::new(),
            system_path: SystemPath::new(Transport::Local, "127.0.0.1".parse().unwrap(), 0),
        }
    }

    fn deadletter_path(&self) -> ActorPath {
        ActorPath::Named(NamedPath::with_system(self.system_path.clone(), Vec::new()))
    }

    fn resolve_path(&self, resolvable: &PathResolvable) -> Result<ActorPath, PathParseError> {
        match resolvable {
            PathResolvable::Path(actor_path) => Ok(actor_path.clone()),
            PathResolvable::Alias(alias) => self
                .system_path
                .clone()
                .into_named_with_string(alias)
                .map(|p| p.into()),
            PathResolvable::Segments(segments) => self
                .system_path
                .clone()
                .into_named_with_vec(segments.to_vec())
                .map(|p| p.into()),
            PathResolvable::ActorId(id) => Ok(self.system_path.clone().into_unique(*id).into()),
            PathResolvable::System => Ok(self.deadletter_path()),
        }
    }

    fn route_local(&mut self, dst: ActorPath, msg: DispatchData) -> () {
        let lookup_result = self.lookup.get_by_actor_path(&dst);
        match msg.into_local() {
            Ok(netmsg) => match lookup_result {
                LookupResult::Ref(actor) => actor.tell(netmsg),
                LookupResult::Group(group) => group.route(netmsg, self.log()),
                LookupResult::None => {
                    warn!(
                        self.ctx.log(),
                        "No local actor found at {:?}. Forwarding to DeadletterBox",
                        netmsg.receiver,
                    );
                    self.ctx.deadletter_ref().enqueue(MsgEnvelope::Net(netmsg));
                }
                LookupResult::Err(e) => {
                    error!(
                        self.ctx.log(),
                        "Local lookup failed at {:?}. Forwarding to DeadletterBox. Error was: {}",
                        netmsg.receiver,
                        e
                    );
                    self.ctx.deadletter_ref().enqueue(MsgEnvelope::Net(netmsg));
                }
            },
            Err(e) => error!(self.log(), "Could not serialise msg: {:?}. Dropping...", e),
        }
    }

    fn register_actor(
        &mut self,
        registration: ActorRegistration,
        update: bool,
        promise: RegistrationPromise,
    ) {
        let ActorRegistration { actor, path } = registration;
        let res = self
            .resolve_path(&path)
            .map_err(RegistrationError::InvalidPath)
            .and_then(|ap| {
                if self.lookup.contains(&path) && !update {
                    Err(RegistrationError::DuplicateEntry)
                } else {
                    self.lookup
                        .insert(path.clone(), actor)
                        .map(|_| ap)
                        .map_err(RegistrationError::InvalidPath)
                }
            });
        match promise {
            RegistrationPromise::Fulfil(promise) => {
                promise.fulfil(res).unwrap_or_else(|e| {
                    error!(self.ctx.log(), "Could not notify listeners: {:?}", e)
                });
            }
            RegistrationPromise::None => {
                trace!(
                    self.ctx.log(),
                    "Actor registration completed without listeners"
                );
            }
        }
    }

    fn register_policy(
        &mut self,
        registration: PolicyRegistration,
        update: bool,
        promise: RegistrationPromise,
    ) {
        let PolicyRegistration { policy, path } = registration;
        let path_res = PathResolvable::Segments(path.clone());
        let res = self
            .resolve_path(&path_res)
            .map_err(RegistrationError::InvalidPath)
            .and_then(|ap| {
                if self.lookup.contains(&path_res) && !update {
                    Err(RegistrationError::DuplicateEntry)
                } else {
                    self.lookup
                        .set_routing_policy(&path, policy)
                        .map(|_| ap)
                        .map_err(RegistrationError::InvalidPath)
                }
            });
        match promise {
            RegistrationPromise::Fulfil(promise) => {
                promise.fulfil(res).unwrap_or_else(|e| {
                    error!(self.ctx.log(), "Could not notify listeners: {:?}", e)
                });
            }
            RegistrationPromise::None => {
                trace!(
                    self.ctx.log(),
                    "Routing policy registration completed without listeners"
                );
            }
        }
    }
}

impl Actor for LocalDispatcher {
    type Message = DispatchEnvelope;

    fn receive_local(&mut self, msg: Self::Message) -> Handled {
        match msg {
            DispatchEnvelope::Msg { dst, msg, .. } => {
                if dst.system() == &self.system_path || dst.protocol() == Transport::Local {
                    self.route_local(dst, msg);
                } else {
                    match msg.into_local() {
                        Ok(netmsg) => {
                            warn!(
                                self.ctx.log(),
                                "Forwarding unresolved remote dispatch to DeadletterBox: {:?}", dst,
                            );
                            self.ctx.deadletter_ref().enqueue(MsgEnvelope::Net(netmsg));
                        }
                        Err(e) => {
                            error!(
                                self.ctx.log(),
                                "Could not serialise remote dispatch to {:?} for deadletters: {:?}",
                                dst,
                                e,
                            );
                        }
                    }
                }
            }
            DispatchEnvelope::ForwardedMsg { msg } => {
                if msg.receiver.system() == &self.system_path
                    || msg.receiver.protocol() == Transport::Local
                {
                    self.route_local(msg.receiver.clone(), DispatchData::NetMessage(msg));
                } else {
                    warn!(
                        self.ctx.log(),
                        "Forwarding unresolved remote dispatch to DeadletterBox: {:?}",
                        msg.receiver,
                    );
                    self.ctx.deadletter_ref().enqueue(MsgEnvelope::Net(msg));
                }
            }
            DispatchEnvelope::Registration(reg) => {
                let RegistrationEnvelope {
                    event,
                    update,
                    promise,
                } = reg;
                match event {
                    RegistrationEvent::Actor(registration) => {
                        self.register_actor(registration, update, promise)
                    }
                    RegistrationEvent::Policy(registration) => {
                        self.register_policy(registration, update, promise)
                    }
                }
            }
            DispatchEnvelope::Event(ev) => {
                warn!(
                    self.ctx.log(),
                    "Ignoring dispatcher event {:?} in LocalDispatcher", ev
                );
            }
            DispatchEnvelope::LockedChunk(_trash) => {
                // Dropping the chunk is sufficient in the local-only dispatcher.
            }
        }
        Handled::Ok
    }

    #[cfg(feature = "distributed")]
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
        self.system_path.clone()
    }
}

impl ComponentLifecycle for LocalDispatcher {
    fn on_start(&mut self) -> Handled {
        debug!(self.ctx.log(), "Starting LocalDispatcher");
        #[cfg(feature = "distributed")]
        self.lookup
            .insert(PathResolvable::System, self.ctx.deadletter_ref().dyn_ref())
            .expect("deadletter path should always be insertable");
        if let Some(promise) = self.notify_ready.take() {
            promise.complete().unwrap_or(())
        }
        Handled::Ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "distributed")]
    use crate::routing::groups::BroadcastRouting;
    use crate::timer::timer_manager::Timer;
    #[cfg(feature = "distributed")]
    use bytes::{Buf, BufMut};
    #[cfg(feature = "distributed")]
    use std::any::Any;
    use std::{sync::mpsc, time::Duration};

    #[derive(ComponentDefinition)]
    struct TimerProbe {
        ctx: ComponentContext<Self>,
        fired: mpsc::Sender<()>,
    }

    impl TimerProbe {
        fn new(fired: mpsc::Sender<()>) -> Self {
            Self {
                ctx: ComponentContext::uninitialised(),
                fired,
            }
        }
    }

    #[cfg(feature = "distributed")]
    #[derive(Debug, Clone, Copy)]
    struct Ping;

    #[cfg(feature = "distributed")]
    impl Serialisable for Ping {
        fn ser_id(&self) -> SerId {
            Self::SER_ID
        }

        fn size_hint(&self) -> Option<usize> {
            Some(0)
        }

        fn serialise(&self, _buf: &mut dyn BufMut) -> Result<(), SerError> {
            Ok(())
        }

        fn local(self: Box<Self>) -> Result<Box<dyn Any + Send>, Box<dyn Serialisable>> {
            Ok(self)
        }

        fn cloned(&self) -> Option<Box<dyn Serialisable>> {
            Some(Box::new(*self))
        }
    }

    #[cfg(feature = "distributed")]
    impl Deserialiser<Ping> for Ping {
        const SER_ID: SerId = 4242;

        fn deserialise(_buf: &mut dyn Buf) -> Result<Ping, SerError> {
            Ok(Ping)
        }
    }

    #[cfg(feature = "distributed")]
    #[derive(ComponentDefinition)]
    struct PathProbe {
        ctx: ComponentContext<Self>,
        received: usize,
        delivered: mpsc::Sender<()>,
    }

    #[cfg(feature = "distributed")]
    impl PathProbe {
        fn new(delivered: mpsc::Sender<()>) -> Self {
            Self {
                ctx: ComponentContext::uninitialised(),
                received: 0,
                delivered,
            }
        }
    }

    #[cfg(feature = "distributed")]
    ignore_lifecycle!(PathProbe);

    #[cfg(feature = "distributed")]
    impl NetworkActor for PathProbe {
        type Deserialiser = Ping;
        type Message = Ping;

        fn receive(&mut self, _sender: Option<ActorPath>, _msg: Self::Message) -> Handled {
            self.received += 1;
            self.delivered
                .send(())
                .expect("probe receiver must stay live");
            Handled::Ok
        }
    }

    #[cfg(feature = "distributed")]
    fn expect_delivery(rx: &mpsc::Receiver<()>) {
        rx.recv_timeout(Duration::from_secs(1))
            .expect("path message should be delivered");
    }

    impl ComponentLifecycle for TimerProbe {
        fn on_start(&mut self) -> Handled {
            let fired = self.fired.clone();
            self.schedule_once(Duration::from_millis(10), move |_component, _timer| {
                fired.send(()).expect("probe receiver must stay live");
                Handled::Ok
            });
            Handled::Ok
        }
    }

    impl Actor for TimerProbe {
        type Message = Never;

        fn receive_local(&mut self, _msg: Self::Message) -> Handled {
            unreachable!("Never type is empty")
        }

        #[cfg(feature = "distributed")]
        fn receive_network(&mut self, _msg: NetMessage) -> Handled {
            unimplemented!("TimerProbe does not use network actor messages")
        }
    }

    #[test]
    fn installed_manual_timer_requires_explicit_advance() {
        let mut config = KompactConfig::default();
        let timer = install_manual_timer(&mut config);
        let system = config.build().expect("build KompactSystem");
        let (tx, rx) = mpsc::channel();
        let component = system.create(|| TimerProbe::new(tx));

        system
            .start_notify(&component)
            .wait_timeout(Duration::from_secs(1))
            .expect("TimerProbe should start");

        assert!(
            rx.recv_timeout(Duration::from_millis(50)).is_err(),
            "manual timer should not fire before explicit advance"
        );

        timer.advance_by(Duration::from_millis(10));
        rx.recv_timeout(Duration::from_secs(1))
            .expect("manual timer should fire after explicit advance");

        system.shutdown().expect("shutdown KompactSystem");
    }

    #[cfg(feature = "distributed")]
    #[test]
    fn local_dispatcher_routes_unique_path_messages() {
        let system = KompactConfig::default()
            .build()
            .expect("build KompactSystem");
        let (tx, rx) = mpsc::channel();
        let probe = system.create(|| PathProbe::new(tx));

        system
            .start_notify(&probe)
            .wait_timeout(Duration::from_secs(1))
            .expect("PathProbe should start");

        let path = system
            .register(&probe)
            .wait_expect(Duration::from_secs(1), "unique registration should succeed");
        path.tell(Ping, &system);

        expect_delivery(&rx);
        probe.on_definition(|cd| assert_eq!(cd.received, 1));
        system.shutdown().expect("shutdown KompactSystem");
    }

    #[cfg(feature = "distributed")]
    #[test]
    fn local_dispatcher_routes_alias_messages() {
        let system = KompactConfig::default()
            .build()
            .expect("build KompactSystem");
        let (tx, rx) = mpsc::channel();
        let probe = system.create(|| PathProbe::new(tx));

        system
            .start_notify(&probe)
            .wait_timeout(Duration::from_secs(1))
            .expect("PathProbe should start");

        let path = system
            .register_by_alias(&probe, "tests/path-probe")
            .wait_expect(Duration::from_secs(1), "alias registration should succeed");
        path.tell(Ping, &system);

        expect_delivery(&rx);
        probe.on_definition(|cd| assert_eq!(cd.received, 1));
        system.shutdown().expect("shutdown KompactSystem");
    }

    #[cfg(feature = "distributed")]
    #[test]
    fn local_dispatcher_routes_broadcast_groups() {
        let system = KompactConfig::default()
            .build()
            .expect("build KompactSystem");
        let (tx1, rx1) = mpsc::channel();
        let (tx2, rx2) = mpsc::channel();
        let probe1 = system.create(|| PathProbe::new(tx1));
        let probe2 = system.create(|| PathProbe::new(tx2));

        system
            .start_notify(&probe1)
            .wait_timeout(Duration::from_secs(1))
            .expect("first PathProbe should start");
        system
            .start_notify(&probe2)
            .wait_timeout(Duration::from_secs(1))
            .expect("second PathProbe should start");

        let group = system
            .set_routing_policy(BroadcastRouting, "tests/group", false)
            .wait_expect(
                Duration::from_secs(1),
                "routing policy registration should succeed",
            );
        system
            .register_by_alias(&probe1, "tests/group/one")
            .wait_expect(
                Duration::from_secs(1),
                "first alias registration should succeed",
            );
        system
            .register_by_alias(&probe2, "tests/group/two")
            .wait_expect(
                Duration::from_secs(1),
                "second alias registration should succeed",
            );

        group.tell(Ping, &system);

        expect_delivery(&rx1);
        expect_delivery(&rx2);
        probe1.on_definition(|cd| assert_eq!(cd.received, 1));
        probe2.on_definition(|cd| assert_eq!(cd.received, 1));
        system.shutdown().expect("shutdown KompactSystem");
    }
}

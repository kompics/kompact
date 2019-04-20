use super::*;
use crate::messaging::DispatchEnvelope;
use bytes::Buf;
use std::any::Any;
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
    fn deadletter_ref(&self) -> ActorRef {
        self.deadletter_box.actor_ref()
    }
    fn dispatcher_ref(&self) -> ActorRef {
        self.dispatcher.actor_ref()
    }
    fn system_path(&self) -> SystemPath {
        self.dispatcher.on_definition(|cd| cd.system_path())
    }
    fn start(&self, system: &KompactSystem) -> () {
        system.start(&self.deadletter_box);
        system.start(&self.dispatcher);
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
    pub(crate) fn new_timer_component() -> Box<TimerComponent> {
        let t = DefaultTimer::new();
        let bt = Box::new(t) as Box<TimerComponent>;
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

pub struct CustomComponents<B, C>
where
    B: ComponentDefinition + Sized + 'static,
    C: ComponentDefinition + Sized + 'static + Dispatcher,
{
    pub(crate) deadletter_box: Arc<Component<B>>,
    pub(crate) dispatcher: Arc<Component<C>>,
}

impl<B, C> SystemComponents for CustomComponents<B, C>
where
    B: ComponentDefinition + Sized + 'static,
    C: ComponentDefinition + Sized + 'static + Dispatcher,
{
    fn deadletter_ref(&self) -> ActorRef {
        self.deadletter_box.actor_ref()
    }
    fn dispatcher_ref(&self) -> ActorRef {
        self.dispatcher.actor_ref()
    }
    fn system_path(&self) -> SystemPath {
        self.dispatcher.on_definition(|cd| cd.system_path())
    }
    fn start(&self, system: &KompactSystem) -> () {
        system.start(&self.deadletter_box);
        system.start(&self.dispatcher);
    }
}

#[derive(ComponentDefinition)]
pub struct DeadletterBox {
    ctx: ComponentContext<DeadletterBox>,
    notify_ready: Option<Promise<()>>,
}

impl DeadletterBox {
    pub fn new(notify_ready: Promise<()>) -> DeadletterBox {
        DeadletterBox {
            ctx: ComponentContext::new(),
            notify_ready: Some(notify_ready),
        }
    }
}

impl Actor for DeadletterBox {
    fn receive_local(&mut self, sender: ActorRef, msg: Box<Any>) -> () {
        info!(
            self.ctx.log(),
            "DeadletterBox received {:?} from {}", msg, sender
        );
    }
    fn receive_message(&mut self, sender: ActorPath, ser_id: u64, _buf: &mut Buf) -> () {
        info!(
            self.ctx.log(),
            "DeadletterBox received buffer with id {:?} from {}", ser_id, sender
        );
    }
}

impl Provide<ControlPort> for DeadletterBox {
    fn handle(&mut self, event: ControlEvent) -> () {
        match event {
            ControlEvent::Start => {
                debug!(self.ctx.log(), "Starting DeadletterBox");
                match self.notify_ready.take() {
                    Some(promise) => promise.fulfill(()).unwrap_or_else(|_| ()),
                    None => (),
                }
            }
            _ => (), // ignore
        }
    }
}

#[derive(ComponentDefinition)]
pub struct LocalDispatcher {
    ctx: ComponentContext<LocalDispatcher>,
    notify_ready: Option<Promise<()>>,
}

impl LocalDispatcher {
    pub fn new(notify_ready: Promise<()>) -> LocalDispatcher {
        LocalDispatcher {
            ctx: ComponentContext::new(),
            notify_ready: Some(notify_ready),
        }
    }
}

impl Actor for LocalDispatcher {
    fn receive_local(&mut self, sender: ActorRef, msg: Box<Any>) -> () {
        info!(
            self.ctx.log(),
            "LocalDispatcher received {:?} from {:?}", msg, sender
        );
    }
    fn receive_message(&mut self, sender: ActorPath, ser_id: u64, _buf: &mut Buf) -> () {
        info!(
            self.ctx.log(),
            "LocalDispatcher received buffer with id {:?} from {:?}", ser_id, sender
        );
    }
}

impl Dispatcher for LocalDispatcher {
    fn receive(&mut self, env: DispatchEnvelope) -> () {
        warn!(
            self.ctx.log(),
            "LocalDispatcher received {:?}, but doesn't know what to do with it (hint: implement dispatching ;)",
            env,
        );
    }
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
                    Some(promise) => promise.fulfill(()).unwrap_or_else(|_| ()),
                    None => (),
                }
            }
            _ => (), // ignore
        }
    }
}

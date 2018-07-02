use super::*;
use bytes::Buf;
use messaging::DispatchEnvelope;
use std::any::Any;
use std::sync::Arc;

pub(crate) struct DefaultComponents {
    deadletter_box: Arc<Component<DeadletterBox>>,
    dispatcher: Arc<Component<LocalDispatcher>>,
}

impl DefaultComponents {
    pub(crate) fn new(system: &KompicsSystem) -> DefaultComponents {
        let dbc = system.create(DeadletterBox::new);
        let ldc = system.create(LocalDispatcher::new);
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
    fn start(&self, system: &KompicsSystem) -> () {
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

// Toooooooo complicated...just do it on demand
//impl<B, C, FB, FC> From<(FB, FC)> for Rc<SCBuilder>
//where
//    B: ComponentDefinition + Sized + 'static,
//    C: ComponentDefinition
//        + Sized
//        + 'static
//        + Dispatcher,
//    FB: Fn() -> B + 'static,
//    FC: Fn() -> C + 'static,
//{
//    fn from(t: (FB, FC)) -> Rc<SCBuilder> {
//        |system: &KompicsSystem| {
//            let dbc = system.create(t.0);
//            let ldc = system.create(t.1);
//            let cc = CustomComponents {
//                deadletter_box: dbc,
//                dispatcher: ldc,
//            };
//            Box::new(cc) as Box<SystemComponents>
//        }
//    }
//}

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
    fn start(&self, system: &KompicsSystem) -> () {
        system.start(&self.deadletter_box);
        system.start(&self.dispatcher);
    }
}

#[derive(ComponentDefinition)]
pub struct DeadletterBox {
    ctx: ComponentContext<DeadletterBox>,
}

impl DeadletterBox {
    pub fn new() -> DeadletterBox {
        DeadletterBox {
            ctx: ComponentContext::new(),
        }
    }
}

impl Actor for DeadletterBox {
    fn receive_local(&mut self, sender: ActorRef, msg: Box<Any>) -> () {
        println!("DeadletterBox received {:?} from {}", msg, sender);
    }
    fn receive_message(&mut self, sender: ActorPath, ser_id: u64, buf: &mut Buf) -> () {
        println!(
            "DeadletterBox received buffer with id {:?} from {}",
            ser_id, sender
        );
    }
}

impl Provide<ControlPort> for DeadletterBox {
    fn handle(&mut self, event: ControlEvent) -> () {
        match event {
            ControlEvent::Start => {
                println!("Starting DeadletterBox");
            }
            _ => (), // ignore
        }
    }
}

#[derive(ComponentDefinition)]
pub struct LocalDispatcher {
    ctx: ComponentContext<LocalDispatcher>,
}

impl LocalDispatcher {
    pub fn new() -> LocalDispatcher {
        LocalDispatcher {
            ctx: ComponentContext::new(),
        }
    }
}

impl Actor for LocalDispatcher {
    fn receive_local(&mut self, sender: ActorRef, msg: Box<Any>) -> () {
        println!("LocalDispatcher received {:?} from {:?}", msg, sender);
    }
    fn receive_message(&mut self, sender: ActorPath, ser_id: u64, buf: &mut Buf) -> () {
        println!(
            "LocalDispatcher received buffer with id {:?} from {:?}",
            ser_id, sender
        );
    }
}

impl Dispatcher for LocalDispatcher {
    fn receive(&mut self, env: DispatchEnvelope) -> () {
        println!(
            "LocalDispatcher received {:?}, but doesn't know what to do with it (hint: implement dispatching ;)",
            env
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
                println!("Starting LocalDispatcher");
            }
            _ => (), // ignore
        }
    }
}

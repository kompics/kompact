use super::*;
use bytes::Buf;
use std::any::Any;
use std::rc::Rc;
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
    ctx: ComponentContext,
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
    ctx: ComponentContext,
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
    fn receive(&mut self, env: SendEnvelope) -> () {
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

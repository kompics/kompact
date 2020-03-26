use kompact::prelude::*;
use std::sync::Arc;

#[derive(ComponentDefinition)]
struct HelloWorldActor {
    ctx: ComponentContext<Self>,
}
impl HelloWorldActor {
    pub fn new() -> Self {
        HelloWorldActor {
            ctx: ComponentContext::new(),
        }
    }
}
ignore_control!(HelloWorldActor);

impl Actor for HelloWorldActor {
    type Message = ();

    fn receive_local(&mut self, _msg: Self::Message) -> () {
        info!(self.ctx.log(), "Hello World!");
        self.ctx().system().shutdown_async();
    }

    fn receive_network(&mut self, _msg: NetMessage) -> () {
        unimplemented!("We are ignoring network messages for now.");
    }
}

pub fn main() {
    let system = KompactConfig::default().build().expect("system");
    let actor: Arc<Component<HelloWorldActor>> = system.create(HelloWorldActor::new);
    system.start(&actor);
    let actor_ref: ActorRef<()> = actor.actor_ref();
    actor_ref.tell(()); // send a unit type message to our actor
    system.await_termination();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_actor_helloworld() {
        main();
    }
}

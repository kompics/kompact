#![allow(clippy::unused_unit)]
// ANCHOR: actor
use kompact::prelude::*;
use std::sync::Arc;

#[derive(ComponentDefinition)]
struct HelloWorldActor {
    ctx: ComponentContext<Self>,
}
impl HelloWorldActor {
    pub fn new() -> Self {
        HelloWorldActor {
            ctx: ComponentContext::uninitialised(),
        }
    }
}
ignore_lifecycle!(HelloWorldActor);

impl Actor for HelloWorldActor {
    type Message = ();

    fn receive_local(&mut self, _msg: Self::Message) -> HandlerResult {
        info!(self.ctx.log(), "Hello World!");
        self.ctx().system().shutdown_async();
        Handled::OK
    }
}
// ANCHOR_END: actor
// ANCHOR: main
pub fn main() {
    let system = KompactConfig::default().build().wait().expect("system");
    let actor: Arc<Component<HelloWorldActor>> = system.create(HelloWorldActor::new);
    system.start(&actor);
    let actor_ref: ActorRef<()> = actor.actor_ref();
    actor_ref.tell(()); // send a unit type message to our actor
    system.await_termination();
}
// ANCHOR_END: main
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_actor_helloworld() {
        kompact::test_support::init_test_logger();
        main();
    }
}

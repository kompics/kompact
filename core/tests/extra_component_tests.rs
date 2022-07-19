use kompact::prelude::*;
use std::time::Duration;

const REPEAT: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelfMessage {
    TellMe,
    RegisterMe,
}

#[derive(ComponentDefinition)]
struct SelfCaller {
    ctx: ComponentContext<Self>,
    counter: usize,
}
impl SelfCaller {
    fn new() -> Self {
        SelfCaller {
            ctx: ComponentContext::uninitialised(),
            counter: 0,
        }
    }

    async fn register_me(&mut self) {
        let self_comp = self.ctx.typed_component();
        info!(self.log(), "Registering me");
        let f = self
            .ctx
            .system()
            .update_alias_registration(&self_comp, "TEST_ME");
        trace!(self.log(), "Awaiting registration response...");
        let path_res = f.await;
        trace!(self.log(), "Got response: {:?}", path_res);
        let path = path_res
            .expect("registration future")
            .expect("registration success");
        info!(self.log(), "Registered me at {}", path);
        self.counter += 1;
        if self.counter < 2 * REPEAT {
            let self_ref: ActorRef<SelfMessage> = self.actor_ref();
            self_ref.tell(SelfMessage::RegisterMe);
            info!(self.log(), "Sent to myself (#{})", self.counter);
        } else {
            info!(self.log(), "Done repeating");
        }
    }

    fn tell_me(&mut self) {
        self.counter += 1;
        info!(self.log(), "Sending to myself (#{})", self.counter);
        if self.counter < REPEAT {
            self.actor_ref().tell(SelfMessage::TellMe);
        } else {
            self.actor_ref().tell(SelfMessage::RegisterMe);
        }
    }
}
info_lifecycle!(SelfCaller);
impl Actor for SelfCaller {
    type Message = SelfMessage;

    fn receive_local(&mut self, msg: Self::Message) -> Handled {
        info!(self.log(), "Got {:?}", msg);
        match msg {
            SelfMessage::TellMe => {
                self.tell_me();
                Handled::Ok
            }
            SelfMessage::RegisterMe => Handled::block_on(self, move |mut async_self| async move {
                async_self.register_me().await;
                info!(async_self.log(), "Done blocking ");
            }),
        }
    }

    fn receive_network(&mut self, _msg: NetMessage) -> Handled {
        unimplemented!("No networking");
    }
}

#[test]
fn repeated_blocking_self_messages() {
    let mut cfg = KompactConfig::default();
    cfg.system_components(DeadletterBox::new, NetworkConfig::default().build());
    let system = cfg.build().expect("system");
    let comp = system.create(SelfCaller::new);
    let reg_f = system.update_alias_registration(&comp, "TEST_ME");
    reg_f.wait_expect(Duration::from_millis(100), "initial registration");
    system.start(&comp);
    comp.actor_ref().tell(SelfMessage::TellMe); // get things started
    let mut done = false;
    while !done {
        std::thread::sleep(Duration::from_millis(100));
        debug!(system.logger(), "Checking component count...");
        let count: usize = comp.on_definition(|cd| cd.counter);
        done = count >= 2 * REPEAT;
        debug!(system.logger(), "Count is {}", count);
    }
    info!(system.logger(), "Test is done");
    system.shutdown().expect("shutdown");
}

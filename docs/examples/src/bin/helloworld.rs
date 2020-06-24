use kompact::prelude::*;

#[derive(ComponentDefinition, Actor)]
struct HelloWorldComponent {
    ctx: ComponentContext<Self>,
}
impl HelloWorldComponent {
    pub fn new() -> Self {
        HelloWorldComponent {
            ctx: ComponentContext::uninitialised(),
        }
    }
}
impl Provide<ControlPort> for HelloWorldComponent {
    fn handle(&mut self, event: ControlEvent) -> Handled {
        match event {
            ControlEvent::Start => {
                info!(self.ctx.log(), "Hello World!");
                self.ctx().system().shutdown_async();
                Handled::Ok
            }
            _ => Handled::Ok, // ignore other control events
        }
    }
}

pub fn main() {
    let system = KompactConfig::default().build().expect("system");
    let component = system.create(HelloWorldComponent::new);
    system.start(&component);
    system.await_termination();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_helloworld() {
        main();
    }
}

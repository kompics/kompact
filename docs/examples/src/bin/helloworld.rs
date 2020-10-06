#![allow(clippy::unused_unit)]
// ANCHOR: component
use kompact::prelude::*;

// ANCHOR: declaration
#[derive(ComponentDefinition, Actor)]
struct HelloWorldComponent {
    ctx: ComponentContext<Self>,
}
// ANCHOR_END: declaration
impl HelloWorldComponent {
    pub fn new() -> Self {
        HelloWorldComponent {
            ctx: ComponentContext::uninitialised(),
        }
    }
}
// ANCHOR: lifecycle
impl ComponentLifecycle for HelloWorldComponent {
    fn on_start(&mut self) -> Handled {
        info!(self.log(), "Hello World!");
        self.ctx.system().shutdown_async();
        Handled::Ok
    }
}
// ANCHOR_END: lifecycle
// ANCHOR_END: component

// ANCHOR: main
pub fn main() {
    let system = KompactConfig::default().build().expect("system");
    // ANCHOR: create
    let component = system.create(HelloWorldComponent::new);
    // ANCHOR_END: create
    system.start(&component);
    system.await_termination();
}
// ANCHOR_END: main

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_helloworld() {
        main();
    }
}

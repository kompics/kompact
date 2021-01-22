use crate::{
    actors::MessageBounds,
    component::{AbstractComponent, ComponentDefinition},
    runtime::KompactSystem,
};
use std::sync::Arc;

/// Trait allowing to create components from type-erased component definitions.
///
/// Should not be implemented manually.
///
/// See: [KompactSystem::create_erased](KompactSystem::create_erased)
pub trait CreateErased<M: MessageBounds> {
    // this is only object-safe with unsized_locals nightly feature
    /// Creates component on the given system.
    fn create_in(self, system: &KompactSystem) -> Arc<dyn AbstractComponent<Message = M>>;
}

impl<M, C> CreateErased<M> for C
where
    M: MessageBounds,
    C: ComponentDefinition<Message = M> + 'static,
{
    fn create_in(self, system: &KompactSystem) -> Arc<dyn AbstractComponent<Message = M>> {
        system.create(|| self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::*;
    use std::time::Duration;

    const WAIT_TIMEOUT: Duration = Duration::from_millis(1000);

    #[derive(ComponentDefinition)]
    struct TestComponent {
        ctx: ComponentContext<Self>,
        counter: u64,
    }

    impl TestComponent {
        fn new() -> TestComponent {
            TestComponent {
                ctx: ComponentContext::uninitialised(),
                counter: 0u64,
            }
        }
    }

    ignore_lifecycle!(TestComponent);

    impl Actor for TestComponent {
        type Message = Ask<u64, ()>;

        fn receive_local(&mut self, msg: Self::Message) -> Handled {
            msg.complete(|num| {
                self.counter += num;
            })
            .expect("Should work!");
            Handled::Ok
        }

        fn receive_network(&mut self, _msg: NetMessage) -> Handled {
            unimplemented!();
        }
    }

    #[test]
    fn test_erased_components() {
        let system = KompactConfig::default().build().expect("System");

        {
            let erased_definition: Box<dyn CreateErased<Ask<u64, ()>>> =
                Box::new(TestComponent::new());
            let erased = system.create_erased(erased_definition);
            let actor_ref = erased.actor_ref();

            let start_f = system.start_notify(&erased);
            start_f.wait_timeout(WAIT_TIMEOUT).expect("Component start");

            let ask_f = actor_ref.ask(42u64);
            ask_f.wait_timeout(WAIT_TIMEOUT).expect("Response");

            erased
                .as_any()
                .downcast_ref::<Component<TestComponent>>()
                .unwrap()
                .on_definition(|c| {
                    assert_eq!(c.counter, 42u64);
                });
        }

        system
            .shutdown()
            .expect("Kompact didn't shut down properly");
    }
}

use super::*;

/// A message type for request-response messages.
///
/// Used together with the [ask](ActorRef::ask) function.
#[derive(Debug)]
pub struct Ask<Request, Response>
where
    Request: MessageBounds,
    Response: Send + Sized,
{
    promise: KPromise<Response>,
    content: Request,
}
impl<Request, Response> Ask<Request, Response>
where
    Request: MessageBounds,
    Response: Send + Sized,
{
    /// Produce a new `Ask` instance from a promise and a `Request`.
    pub fn new(promise: KPromise<Response>, content: Request) -> Ask<Request, Response> {
        Ask { promise, content }
    }

    /// Produce a function that takes a promise and returns an ask with the given `Request`.
    ///
    /// Use this avoid the explicit chaining of the `promise` that the [new](Ask::new) function requires.
    pub fn of(content: Request) -> impl FnOnce(KPromise<Response>) -> Ask<Request, Response> {
        |promise| Ask::new(promise, content)
    }

    /// The request associated with this `Ask`.
    pub fn request(&self) -> &Request {
        &self.content
    }

    /// The request associated with this `Ask` (mutable).
    pub fn request_mut(&mut self) -> &mut Request {
        &mut self.content
    }

    /// Decompose this `Ask` into a pair of a promise and a `Request`.
    pub fn take(self) -> (KPromise<Response>, Request) {
        (self.promise, self.content)
    }

    /// Reply to this `Ask` with the `response`.
    ///
    /// Fails with a [PromiseErr](PromiseErr) if the promise has already been fulfilled or the other end dropped the future.
    pub fn reply(self, response: Response) -> Result<(), PromiseErr> {
        self.promise.fulfil(response)
    }

    /// Run `f` to produce a response to this `Ask` and reply with that reponse.
    ///
    /// Fails with a [PromiseErr](PromiseErr) if the promise has already been fulfilled or the other end dropped the future.
    pub fn complete<F>(self, f: F) -> Result<(), PromiseErr>
    where
        F: FnOnce(Request) -> Response,
    {
        let response = f(self.content);
        self.promise.fulfil(response)
    }

    /// Run the future produced by `f` to completion, and then reply with its result.
    ///
    /// Fails with a [PromiseErr](PromiseErr) if the promise has already been fulfilled or the other end dropped the future.
    pub async fn complete_with<F>(self, f: impl FnOnce(Request) -> F) -> Result<(), PromiseErr>
    where
        F: Future<Output = Response> + Send + 'static,
    {
        let response = f(self.content).await;
        self.promise.fulfil(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messaging::NetMessage;

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
    fn test_ask_complete() -> () {
        let system = KompactConfig::default().build().expect("System");
        let tc = system.create(TestComponent::new);
        let tc_ref = tc.actor_ref();
        let tc_sref = tc_ref.hold().expect("Live ref!");

        let start_f = system.start_notify(&tc);
        start_f.wait_timeout(WAIT_TIMEOUT).expect("Component start");

        let ask_f = tc_ref.ask(|promise| Ask::new(promise, 42u64));
        ask_f.wait_timeout(WAIT_TIMEOUT).expect("Response");

        tc.on_definition(|c| {
            assert_eq!(c.counter, 42u64);
        });

        let ask_f2 = tc_sref.ask(Ask::of(1u64));
        ask_f2.wait_timeout(WAIT_TIMEOUT).expect("Response2");

        tc.on_definition(|c| {
            assert_eq!(c.counter, 43u64);
        });

        drop(tc_ref);
        drop(tc_sref);
        drop(tc);
        system
            .shutdown()
            .expect("Kompact didn't shut down properly");
    }

    #[derive(ComponentDefinition)]
    struct AsyncTestComponent {
        ctx: ComponentContext<Self>,
        proxee: ActorRef<Ask<u64, ()>>,
        mode: AsyncMode,
    }

    impl AsyncTestComponent {
        fn new(proxee: ActorRef<Ask<u64, ()>>, mode: AsyncMode) -> AsyncTestComponent {
            AsyncTestComponent {
                ctx: ComponentContext::uninitialised(),
                proxee,
                mode,
            }
        }
    }

    ignore_lifecycle!(AsyncTestComponent);

    impl Actor for AsyncTestComponent {
        type Message = Ask<u64, ()>;

        fn receive_local(&mut self, msg: Self::Message) -> Handled {
            match self.mode {
                AsyncMode::Blocking => Handled::block_on(self, move |async_self| async move {
                    msg.complete_with(move |num| async move {
                        async_self.proxee.ask(Ask::of(num)).await.expect("result");
                    })
                    .await
                    .expect("complete");
                }),
                AsyncMode::SpawnOff => {
                    let proxee = self.proxee.clone();
                    let handle = self.spawn_off(async move {
                        msg.complete_with(move |num| async move {
                            proxee.ask(Ask::of(num)).await.expect("result");
                        })
                        .await
                        .expect("complete");
                    });
                    drop(handle);
                    Handled::Ok
                }
                AsyncMode::SpawnLocal => {
                    self.spawn_local(move |async_self| async move {
                        let proxee = async_self.proxee.clone();
                        let res = msg
                            .complete_with(move |num| async move {
                                proxee.ask(Ask::of(num)).await.expect("result");
                            })
                            .await;
                        if let Err(err) = res {
                            error!(async_self.log(), "Could not complete request: {}", err);
                        }
                        Handled::Ok
                    });
                    Handled::Ok
                }
            }
        }

        fn receive_network(&mut self, _msg: NetMessage) -> Handled {
            unimplemented!();
        }
    }

    enum AsyncMode {
        Blocking,
        SpawnOff,
        SpawnLocal,
    }

    #[test]
    fn test_ask_complete_with_blocking() -> () {
        test_ask_complete_with(AsyncMode::Blocking)
    }

    #[test]
    fn test_ask_complete_with_spawn_off() -> () {
        test_ask_complete_with(AsyncMode::SpawnOff)
    }

    #[test]
    fn test_ask_complete_with_spawn_local() -> () {
        test_ask_complete_with(AsyncMode::SpawnLocal)
    }

    fn test_ask_complete_with(mode: AsyncMode) -> () {
        let system = KompactConfig::default().build().expect("System");
        {
            let tc = system.create(TestComponent::new);
            let tc_ref = tc.actor_ref();
            let atc = system.create(move || AsyncTestComponent::new(tc_ref, mode));
            let atc_ref = atc.actor_ref();

            system
                .start_notify(&tc)
                .wait_timeout(WAIT_TIMEOUT)
                .expect("Component start");
            system
                .start_notify(&atc)
                .wait_timeout(WAIT_TIMEOUT)
                .expect("Component start");

            let ask_f = atc_ref.ask(|promise| Ask::new(promise, 42u64));
            ask_f.wait_timeout(WAIT_TIMEOUT).expect("Response");

            tc.on_definition(|c| {
                assert_eq!(c.counter, 42u64);
            });

            let ask_f2 = atc_ref.ask(Ask::of(1u64));
            ask_f2.wait_timeout(WAIT_TIMEOUT).expect("Response2");

            tc.on_definition(|c| {
                assert_eq!(c.counter, 43u64);
            });
        }
        system
            .shutdown()
            .expect("Kompact didn't shut down properly");
    }
}

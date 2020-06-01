use std::sync::Arc;

use std::{
    fmt,
    ops::DerefMut,
    sync::{mpsc, TryLockError},
    time::Duration,
};

use super::*;

/// This error type is returned when on [`on_dual_definition`](fn.on_dual_definition.html) fails,
/// indicating that a proper lock could not be established on both components.
#[derive(Debug)]
pub enum TryDualLockError {
    /// Indicates that the first component would block while waiting to be locked.
    LeftWouldBlock,
    /// Indicates that the second component would block while waiting to be locked.
    RightWouldBlock,
    /// Indicates that the first component has had its mutex poisoned.
    LeftPoisoned,
    /// Indicates that the second component has had its mutex poisoned.
    RightPoisoned,
}

/// Applies function `f` to the component definitions of `c1` and `c2`, while holding both mutexes.
///
/// As locking two mutexes is a deadlock risk, this method fails immediately if one of the mutexes can not be acquired.
///
/// This method is used internally by [`biconnect_components`](prelude/fn.biconnect_components.html) and can be used
/// directly for the same purpose, if some additional work needs to be done on one or both component definitions and
/// it is desirable to avoid acquiring the same mutex again.
///
/// This function can fail with a [`TryDualLockError`](enum.TryDualLockError.html), if one of the mutexes can not be acquired immediately.
pub fn on_dual_definition<C1, C2, F, T>(
    c1: &Arc<Component<C1>>,
    c2: &Arc<Component<C2>>,
    f: F,
) -> Result<T, TryDualLockError>
where
    C1: ComponentDefinition + Sized + 'static,
    C2: ComponentDefinition + Sized + 'static,
    F: FnOnce(&mut C1, &mut C2) -> T,
{
    //c1.on_definition(|cd1| c2.on_definition(|cd2| f(cd1, cd2)))
    let mut cd1 = c1.definition().try_lock().map_err(|e| match e {
        TryLockError::Poisoned(_) => TryDualLockError::LeftPoisoned,
        TryLockError::WouldBlock => TryDualLockError::LeftWouldBlock,
    })?;
    let mut cd2 = c2.definition().try_lock().map_err(|e| match e {
        TryLockError::Poisoned(_) => TryDualLockError::RightPoisoned,
        TryLockError::WouldBlock => TryDualLockError::RightWouldBlock,
    })?;
    Ok(f(cd1.deref_mut(), cd2.deref_mut()))
}

/// Connect two components on their instances of port type `P`.
///
/// The component providing the port must be given as first argument, and the requiring component second.
///
/// This function can fail with a [`TryDualLockError`](enum.TryDualLockError.html), if one of the mutexes can not be acquired immediately.
///
/// # Example
///
/// ```
/// use kompact::prelude::*;
/// # use kompact::doctest_helpers::*;
///
///
/// let system = KompactConfig::default().build().expect("system");
/// let c1 = system.create(TestComponent1::new);
/// let c2 = system.create(TestComponent2::new);
/// biconnect_components::<TestPort,_,_>(&c1, &c2).expect("connection");
/// ```
pub fn biconnect_components<P, C1, C2>(
    provider: &Arc<Component<C1>>,
    requirer: &Arc<Component<C2>>,
) -> Result<(), TryDualLockError>
where
    P: Port + 'static,
    C1: ComponentDefinition + Sized + 'static + Provide<P> + ProvideRef<P>,
    C2: ComponentDefinition + Sized + 'static + Require<P> + RequireRef<P>,
{
    on_dual_definition(provider, requirer, |prov, req| {
        let prov_share: ProvidedRef<P> = prov.provided_ref();
        let req_share: RequiredRef<P> = req.required_ref();
        ProvideRef::connect_to_required(prov, req_share);
        RequireRef::connect_to_provided(req, prov_share);
    })
}

/// Connect two port instances.
///
/// The providing port instance must be given as first argument, and the requiring instance second.
pub fn biconnect_ports<P, C1, C2>(
    prov: &mut ProvidedPort<P, C1>,
    req: &mut RequiredPort<P, C2>,
) -> ()
where
    P: Port,
    C1: ComponentDefinition + Sized + 'static + Provide<P>,
    C2: ComponentDefinition + Sized + 'static + Require<P>,
{
    let prov_share = prov.share();
    let req_share = req.share();
    prov.connect(req_share);
    req.connect(prov_share);
}

/// Produces a new `Promise`/`Future` pair.
///
/// # Note
/// This API is considered temporary and will eventually be replaced with Rust's async facilities.
pub fn promise<T: Send + Sized>() -> (Promise<T>, Future<T>) {
    let (tx, rx) = mpsc::channel();
    let f = Future { result_channel: rx };
    let p = Promise { result_channel: tx };
    (p, f)
}

/// An error returned when a promise can't be fulfilled.
#[derive(Debug)]
pub enum PromiseErr {
    /// Indicates that the paired future was already dropped.
    ChannelBroken,
    /// Indicates that this promise has somehow been fulfilled before.
    AlreadyFulfilled,
}

/// A custom future implementation, that can be fulfilled via its paired promise.
///
/// # Note
/// This API is considered temporary and will eventually be replaced with Rust's async facilities.
#[derive(Debug)]
pub struct Future<T: Send + Sized> {
    result_channel: mpsc::Receiver<T>,
}

impl<T: Send + Sized> Future<T> {
    /// Wait for the future to be fulfilled and return the value.
    ///
    /// This method panics, if there is an error with the link to the promise.
    pub fn wait(self) -> T {
        self.result_channel.recv().unwrap()
    }

    /// Wait for the future to be fulfilled or the timeout to expire.
    ///
    /// If the timeout expires, the future itself is returned, so it can be retried later.
    pub fn wait_timeout(self, timeout: Duration) -> Result<T, Future<T>> {
        self.result_channel.recv_timeout(timeout).map_err(|_| self)
    }
}
impl<T: Send + Sized + fmt::Debug, E: Send + Sized + fmt::Debug> Future<Result<T, E>> {
    /// Wait for the future to be fulfilled or the timeout to expire.
    ///
    /// If the result of the future is an error or the the timeout expires,
    /// this method will panic with an appropriate error message.
    pub fn wait_expect(self, timeout: Duration, error_msg: &'static str) -> T {
        self.wait_timeout(timeout)
            .expect(&format!("{} (caused by timeout)", error_msg))
            .expect(&format!("{} (caused by result)", error_msg))
    }
}

/// Anything that can be fulfilled with a value of type `T`.
pub trait Fulfillable<T> {
    /// Fulfil self with the value `t`
    ///
    /// Returns a [PromiseErr](PromiseErr) if unsuccessful.
    fn fulfil(self, t: T) -> Result<(), PromiseErr>;
}

/// A custom promise implementation, that can be used to fulfil its paired future.
///
/// # Note
/// This API is considered temporary and will eventually be replaced with Rust's async facilities.
#[derive(Debug, Clone)]
pub struct Promise<T: Send + Sized> {
    result_channel: mpsc::Sender<T>,
}

impl<T: Send + Sized> Fulfillable<T> for Promise<T> {
    fn fulfil(self, t: T) -> Result<(), PromiseErr> {
        self.result_channel
            .send(t)
            .map_err(|_| PromiseErr::ChannelBroken)
    }
}

/// A message type for request-response messages.
///
/// Used together with the [ask](ActorRef::ask) function.
#[derive(Debug)]
pub struct Ask<Request, Response>
where
    Request: MessageBounds,
    Response: Send + Sized,
{
    promise: Promise<Response>,
    content: Request,
}
impl<Request, Response> Ask<Request, Response>
where
    Request: MessageBounds,
    Response: Send + Sized,
{
    /// Produce a new `Ask` instance from a promise and a `Request`.
    pub fn new(promise: Promise<Response>, content: Request) -> Ask<Request, Response> {
        Ask { promise, content }
    }

    /// Produce a function that takes a promise and returns an ask with the given `Request`.
    ///
    /// Use this avoid the explicit chaining of the `promise` that the [new](Ask::new) function requires.
    pub fn of(content: Request) -> impl FnOnce(Promise<Response>) -> Ask<Request, Response> {
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
    pub fn take(self) -> (Promise<Response>, Request) {
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
}

/// A macro that provides an empty implementation of the [ControlPort](ControlPort) handler.
///
/// Use this in components that do not require any special treatment of control events.
///
/// # Example
///
/// To ignore control events for a component `TestComponent`, write:
/// `ignore_control!(TestComponent);`
#[macro_export]
macro_rules! ignore_control {
    ($component:ty) => {
        ignore_requests!(ControlPort, $component);
    };
}
// macro_rules! ignore_control {
//     ($component:ty) => {
//         impl Provide<ControlPort> for $component {
//             fn handle(&mut self, _event: ControlEvent) -> () {
//                 () // ignore all
//             }
//         }
//     };
// }

/// A macro that provides an empty implementation of the provided handler for the given `$port` on the given `$component`
///
/// Use this in components that do not require any treatment of requests on `$port`, for example
/// where `$port::Request` is the uninhabited `Never` type.
///
/// # Example
///
/// To ignore requests on a port `TestPort` for a component `TestComponent`, write:
/// `ignore_requests!(TestPort, TestComponent);`
#[macro_export]
macro_rules! ignore_requests {
    ($port:ty, $component:ty) => {
        impl Provide<$port> for $component {
            fn handle(&mut self, _event: <$port as Port>::Request) -> () {
                () // ignore all
            }
        }
    };
}

/// A macro that provides an empty implementation of the required handler for the given `$port` on the given `$component`
///
/// Use this in components that do not require any treatment of indications on `$port`, for example
/// where `$port::Indication` is the uninhabited `Never` type.
///
/// # Example
///
/// To ignore indications on a port `TestPort` for a component `TestComponent`, write:
/// `ignore_indications!(TestPort, TestComponent);`
#[macro_export]
macro_rules! ignore_indications {
    ($port:ty, $component:ty) => {
        impl Require<$port> for $component {
            fn handle(&mut self, _event: <$port as Port>::Indication) -> () {
                () // ignore all
            }
        }
    };
}

/// Additional iterator functions
pub trait IterExtras: Iterator + Sized {
    // this variant requires #![feature(unsized_locals)]
    //pub trait IterExtras: Iterator {

    /// Iterate over each item in the iterator and apply a function to it and a clone of the given value `t`
    ///
    /// Behaves like `iterator.for_each(|item| f(item, t.clone()))`, except that it avoids cloning
    /// in the case where the iterator contains a single item or for the last item in a larger iterator.
    ///
    /// Use this when cloning `T` is relatively expensive compared to `f`.
    fn for_each_with<T, F>(mut self, t: T, mut f: F)
    where
        T: Clone,
        F: FnMut(Self::Item, T),
    {
        let mut current: Option<Self::Item> = self.next();
        let mut next: Option<Self::Item> = self.next();
        while next.is_some() {
            let item = current.take().unwrap();
            f(item, t.clone());
            current = next;
            next = self.next();
        }
        match current.take() {
            Some(item) => f(item, t),
            None => (), // nothing to do
        }
    }
}
impl<T: Sized> IterExtras for T where T: Iterator {}
// this variant requires #![feature(unsized_locals)]
//impl<T: ?Sized> IterExtras for T where T: Iterator {}

#[cfg(all(nightly, feature = "type_erasure"))]
pub mod erased {
    use crate::{
        actors::{ActorRefFactory, MessageBounds},
        component::{Component, ComponentDefinition, CoreContainer},
        lifecycle::ControlEvent,
        runtime::KompactSystem,
    };
    use std::{any::Any, sync::Arc};

    /// Trait representing a type-erased component.
    pub trait ErasedComponent: ActorRefFactory + CoreContainer + Any {
        fn enqueue_control(&self, event: ControlEvent);
        fn as_any(&self) -> &dyn Any;
    }

    impl<C> ErasedComponent for Component<C>
    where
        C: ComponentDefinition,
    {
        fn enqueue_control(&self, event: ControlEvent) {
            Component::enqueue_control(self, event)
        }

        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    /// Trait allowing to create components from type-erased definitions.
    ///
    /// Should not be implemented manually.
    ///
    /// See: [KompactSystem::create_erased](KompactSystem::create_erased)
    pub trait ErasedComponentDefinition<M: MessageBounds> {
        // this is only object-safe with unsized_locals nightly feature
        /// Creates component on the given system.
        fn spawn_on(self, system: &KompactSystem) -> Arc<dyn ErasedComponent<Message = M>>;
    }

    impl<M, C> ErasedComponentDefinition<M> for C
    where
        M: MessageBounds,
        C: ComponentDefinition<Message = M> + 'static,
    {
        fn spawn_on(self, system: &KompactSystem) -> Arc<dyn ErasedComponent<Message = M>> {
            system.create(|| self)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messaging::NetMessage;

    #[derive(ComponentDefinition)]
    struct TestComponent {
        ctx: ComponentContext<Self>,
        counter: u64,
    }

    impl TestComponent {
        fn new() -> TestComponent {
            TestComponent {
                ctx: ComponentContext::new(),
                counter: 0u64,
            }
        }
    }

    ignore_control!(TestComponent);

    impl Actor for TestComponent {
        type Message = Ask<u64, ()>;

        fn receive_local(&mut self, msg: Self::Message) -> () {
            msg.complete(|num| {
                self.counter += num;
            })
            .expect("Should work!");
        }

        fn receive_network(&mut self, _msg: NetMessage) -> () {
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
        start_f
            .wait_timeout(Duration::from_millis(1000))
            .expect("Component start");

        let ask_f = tc_ref.ask(|promise| Ask::new(promise, 42u64));
        ask_f
            .wait_timeout(Duration::from_millis(1000))
            .expect("Response");

        tc.on_definition(|c| {
            assert_eq!(c.counter, 42u64);
        });

        let ask_f2 = tc_sref.ask(Ask::of(1u64));
        ask_f2
            .wait_timeout(Duration::from_millis(1000))
            .expect("Response2");

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

    #[cfg(all(nightly, feature = "type_erasure"))]
    #[test]
    fn test_erased_components() {
        use utils::erased::ErasedComponentDefinition;
        let system = KompactConfig::default().build().expect("System");

        {
            let erased_definition: Box<dyn ErasedComponentDefinition<Ask<u64, ()>>> =
                Box::new(TestComponent::new());
            let erased = system.create_erased(erased_definition);
            let actor_ref = erased.actor_ref();

            let start_f = system.start_erased_notify(&erased);
            start_f
                .wait_timeout(Duration::from_millis(1000))
                .expect("Component start");

            let ask_f = actor_ref.ask(|promise| Ask::new(promise, 42u64));
            ask_f
                .wait_timeout(Duration::from_millis(1000))
                .expect("Response");

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

use std::sync::Arc;

use futures::channel::oneshot;
use std::{
    error,
    fmt,
    future::Future,
    iter::FromIterator,
    pin::Pin,
    sync::TryLockError,
    task::{Context, Poll},
    time::Duration,
};

use super::*;

pub use iter_extras::*;

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
    let mut cd1 = c1.mutable_core.try_lock().map_err(|e| match e {
        TryLockError::Poisoned(_) => TryDualLockError::LeftPoisoned,
        TryLockError::WouldBlock => TryDualLockError::LeftWouldBlock,
    })?;
    let mut cd2 = c2.mutable_core.try_lock().map_err(|e| match e {
        TryLockError::Poisoned(_) => TryDualLockError::RightPoisoned,
        TryLockError::WouldBlock => TryDualLockError::RightWouldBlock,
    })?;
    Ok(f(&mut cd1.definition, &mut cd2.definition))
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
) -> Result<TwoWayChannel<P, C1, C2>, TryDualLockError>
where
    P: Port + 'static,
    C1: ComponentDefinition + Sized + 'static + Provide<P> + ProvideRef<P>,
    C2: ComponentDefinition + Sized + 'static + Require<P> + RequireRef<P>,
{
    let (provided_ref, required_ref) = on_dual_definition(provider, requirer, |prov, req| {
        let prov_share: ProvidedRef<P> = prov.provided_ref();
        let req_share: RequiredRef<P> = req.required_ref();
        ProvideRef::connect_to_required(prov, req_share.clone());
        RequireRef::connect_to_provided(req, prov_share.clone());
        (prov_share, req_share)
    })?;
    Ok(TwoWayChannel::new(
        provider,
        requirer,
        provided_ref,
        required_ref,
    ))
}

/// Connect two port instances.
///
/// The providing port instance must be given as first argument, and the requiring instance second.
pub fn biconnect_ports<P: Port>(prov: &mut ProvidedPort<P>, req: &mut RequiredPort<P>) -> () {
    let prov_share = prov.share();
    let req_share = req.share();
    prov.connect(req_share);
    req.connect(prov_share);
}

/// Produces a new `KPromise`/`KFuture` pair.
pub fn promise<T: Send + Sized>() -> (KPromise<T>, KFuture<T>) {
    let (tx, rx) = oneshot::channel();
    let f = KFuture { result_channel: rx };
    let p = KPromise { result_channel: tx };
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
impl error::Error for PromiseErr {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        None
    }
}
impl fmt::Display for PromiseErr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PromiseErr::ChannelBroken => write!(f, "The Future corresponding to this Promise was dropped without waiting for completion first."),
            PromiseErr::AlreadyFulfilled => write!(f, "This Promise has already been fulfilled. Double fulfilling a promise is illegal."),
        }
    }
}

/// A custom future implementation, that can be fulfilled via its paired promise.
#[derive(Debug)]
pub struct KFuture<T: Send + Sized> {
    result_channel: oneshot::Receiver<T>,
}

#[derive(Debug, Clone)]
pub struct PromiseDropped;
impl error::Error for PromiseDropped {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        None
    }
}
impl fmt::Display for PromiseDropped {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "The Promise corresponding to this Future was dropped without completing first."
        )
    }
}

impl<T: Send + Sized> Future for KFuture<T> {
    type Output = Result<T, PromiseDropped>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        unsafe {
            self.map_unchecked_mut(|s| &mut s.result_channel)
                .poll(cx)
                .map(|res| res.map_err(|_e| PromiseDropped))
        }
    }
}

pub enum WaitErr<T> {
    /// A timeout occurred and the original `T`
    /// that was waited on is returned
    Timeout(T),
    /// The promise that was waited on was dropped
    ///
    /// Since the `T` can never be completed now, it is not returned.
    PromiseDropped(PromiseDropped),
}
impl<T> error::Error for WaitErr<T> {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            WaitErr::Timeout(_) => None,
            WaitErr::PromiseDropped(ref p) => Some(p),
        }
    }
}
impl<T> fmt::Debug for WaitErr<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WaitErr::Timeout(_) => write!(f, "WaitErr::Timeout(<some future>)"),
            WaitErr::PromiseDropped(ref p) => fmt::Debug::fmt(p, f),
        }
    }
}
impl<T> fmt::Display for WaitErr<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WaitErr::Timeout(_) => write!(f, "The timeout expired."),
            WaitErr::PromiseDropped(ref p) => fmt::Display::fmt(p, f),
        }
    }
}

impl<T: Send + Sized> KFuture<T> {
    /// Wait for the future to be fulfilled and return the value.
    ///
    /// This method panics, if there is an error with the link to the promise.
    pub fn wait(self) -> T {
        futures::executor::block_on(self.result_channel).unwrap()
    }

    /// Wait for the future to be fulfilled or the timeout to expire.
    ///
    /// If the timeout expires, the future itself is returned, so it can be retried later.
    pub fn wait_timeout(self, timeout: Duration) -> Result<T, WaitErr<Self>> {
        block_until(timeout, self)
            .map_err(WaitErr::Timeout)
            .and_then(|res| res.map_err(WaitErr::PromiseDropped))
    }
}
impl<T: Send + Sized + fmt::Debug, E: Send + Sized + fmt::Debug> KFuture<Result<T, E>> {
    /// Wait for the future to be fulfilled or the timeout to expire.
    ///
    /// If the result of the future is an error or the the timeout expires,
    /// this method will panic with an appropriate error message.
    pub fn wait_expect(self, timeout: Duration, error_msg: &'static str) -> T {
        self.wait_timeout(timeout)
            .unwrap_or_else(|e| panic!("{}\n    Error was caused by timeout: {:?}", error_msg, e))
            .unwrap_or_else(|e| panic!("{}\n    Error was caused by result: {:?}", error_msg, e))
    }
}

pub use futures::executor::block_on;

/// Blocks the current waiting for `f` to be completed or for the `timeout` to expire
///
/// If the timeout expires first, then `f` is returned so it can be retried if desired.
pub fn block_until<F>(timeout: Duration, mut f: F) -> Result<<F as Future>::Output, F>
where
    F: Future + Unpin,
{
    block_on(async {
        async_std::future::timeout(timeout, &mut f)
            .await
            .map_err(|_| ())
    })
    .map_err(|_| f)
}

/// Anything that can be fulfilled with a value of type `T`.
pub trait Fulfillable<T> {
    /// Fulfil self with the value `t`
    ///
    /// Returns a [PromiseErr](PromiseErr) if unsuccessful.
    fn fulfil(self, t: T) -> Result<(), PromiseErr>;
}

/// A custom promise implementation, that can be used to fulfil its paired future.
#[derive(Debug)]
pub struct KPromise<T: Send + Sized> {
    result_channel: oneshot::Sender<T>,
}

impl<T: Send + Sized> Fulfillable<T> for KPromise<T> {
    fn fulfil(self, t: T) -> Result<(), PromiseErr> {
        self.result_channel
            .send(t)
            .map_err(|_| PromiseErr::ChannelBroken)
    }
}

/// Convenience methods for collections containing futures
pub trait FutureCollection<T> {
    /// Wait for all futures to complete successfully but ignore the result
    ///
    /// Panics if any future fails to complete in time.
    /// Panics use the provided `error_msg`.
    ///
    /// Timeout values are per future, not overall.
    fn expect_completion(self, timeout: Duration, error_msg: &'static str) -> ();

    /// Collect all the future results into a collection of type `B`
    ///
    /// Returns an `Err` variant if any future fails to complete before `timeout` expires.
    ///
    /// Timeout values are per future, not overall.
    fn collect_with_timeout<B>(self, timeout: Duration) -> Result<B, WaitErr<()>>
    where
        B: FromIterator<T>;

    /// Collect all the future results into a collection of type `B`
    ///
    /// Panics if any future fails to complete.
    fn collect_results<B>(self) -> B
    where
        B: FromIterator<T>;
}
/// Convenience methods for collections containing future results
pub trait FutureResultCollection<T, E>: FutureCollection<Result<T, E>> {
    /// Wait for all futures to complete successfully but ignore the result
    ///
    /// Panics if any future fails to complete in time or the result is `Err`.
    /// Panics use the provided `error_msg`.
    ///
    /// Timeout values are per future, not overall.
    fn expect_ok(self, timeout: Duration, error_msg: &'static str) -> ();

    /// Collect all the future results into a collection of type `B`
    ///
    /// Panics if any future fails to complete or the result is `Err`.
    fn collect_ok<B>(self) -> B
    where
        B: FromIterator<T>;

    /// Collect all the future results into a collection of type `B`
    ///
    /// Panics if any future fails to complete in time or the result is `Err`.
    /// Panics use the provided `error_msg`.
    ///
    /// Timeout values are per future, not overall.
    fn collect_ok_with_timeout<B>(self, timeout: Duration, error_msg: &'static str) -> B
    where
        B: FromIterator<T>;
}

impl<I, T: Send + Sized> FutureCollection<T> for I
where
    I: IntoIterator<Item = KFuture<T>>,
{
    fn expect_completion(self, timeout: Duration, error_msg: &'static str) -> () {
        for f in self {
            let _ = f.wait_timeout(timeout).expect(error_msg);
        }
    }

    fn collect_with_timeout<B>(self, timeout: Duration) -> Result<B, WaitErr<()>>
    where
        B: FromIterator<T>,
    {
        let mut temp: Vec<T> = Vec::new();
        for f in self {
            let v = f.wait_timeout(timeout).map_err(|_| WaitErr::Timeout(()))?;
            temp.push(v);
        }
        Ok(temp.into_iter().collect())
    }

    fn collect_results<B>(self) -> B
    where
        B: FromIterator<T>,
    {
        self.into_iter().map(|f| f.wait()).collect()
    }
}

impl<I, T, E> FutureResultCollection<T, E> for I
where
    T: Send + Sized + fmt::Debug,
    E: Send + Sized + fmt::Debug,
    I: IntoIterator<Item = KFuture<Result<T, E>>>,
{
    fn expect_ok(self, timeout: Duration, error_msg: &'static str) -> () {
        for f in self {
            let _ = f.wait_expect(timeout, error_msg);
        }
    }

    fn collect_ok<B>(self) -> B
    where
        B: FromIterator<T>,
    {
        self.into_iter().map(|f| f.wait().unwrap()).collect()
    }

    fn collect_ok_with_timeout<B>(self, timeout: Duration, error_msg: &'static str) -> B
    where
        B: FromIterator<T>,
    {
        self.into_iter()
            .map(|f| f.wait_expect(timeout, error_msg))
            .collect()
    }
}

//Vec<KFuture<RegistrationResult>>

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
    /// # Note
    ///
    /// The [PromiseErr](PromiseErr) that can be produced during reply attempts is silently ignored in this method.
    /// If you need to handle reply errors, use [complete_with_err](complete_with_err) instead.
    pub async fn complete_with<F>(self, f: impl FnOnce(Request) -> F) -> () where F: Future<Output=Response> + Send + 'static {
        let response = f(self.content).await;
        let _ = self.promise.fulfil(response); // ignore promise errors, since we got nothing to do with them
    }

    /// Run the future produced by `f` to completion, and then reply with its result. 
    ///
    /// This is the same as [complete_with](complete_with), but you can provide a custom error handling function.
    pub async fn complete_with_err<F>(self, f: impl FnOnce(Request) -> F, err: impl FnOnce(PromiseErr) -> ()) -> () where F: Future<Output=Response> + Send + 'static {
        let response = f(self.content).await;
        let res = self.promise.fulfil(response); // ignore promise errors, since we got nothing to do with them
        if let Err(e) = res {
            err(e);
        }
    }
}

/// A macro that provides a shorthand for an irrefutable let binding,
/// that the compiler cannot determine on its own
///
/// This is equivalent to the following code:
/// `let name = if let Pattern(name) = expression { name } else { unreachable!(); };`
#[macro_export]
macro_rules! let_irrefutable {
    ($name:ident, $pattern:pat = $expression:expr) => {
        let $name = if let $pattern = $expression {
            $name
        } else {
            unreachable!("Pattern is irrefutable!");
        };
    };
}

/// A macro that provides an empty implementation of [ComponentLifecycle](ComponentLifecycle) for the given component
///
/// Use this in components that do not require any special treatment of lifecycle events.
///
/// # Example
///
/// To ignore lifecycle events for a component `TestComponent`, write:
/// `ignore_lifecycle!(TestComponent);`
///
/// This is equivalent to `impl ComponentLifecycle for TestComponent {}`
/// and uses the default implementations for each trait function.
#[macro_export]
macro_rules! ignore_lifecycle {
    ($component:ty) => {
        impl ComponentLifecycle for $component {}
    };
}

/// A macro that provides an implementation of [ComponentLifecycle](ComponentLifecycle) for the given component that logs
/// the lifecycle stages at INFO log level.
///
/// Use this in components that do not require any special treatment of lifecycle events besides logging.
///
/// # Example
///
/// To log lifecycle events for a component `TestComponent`, write:
/// `info_lifecycle!(TestComponent);`
#[macro_export]
macro_rules! info_lifecycle {
    ($component:ty) => {
        impl ComponentLifecycle for $component {
            fn on_start(&mut self) -> Handled {
                info!(self.log(), "Starting...");
                Handled::Ok
            }

            fn on_stop(&mut self) -> Handled {
                info!(self.log(), "Stopping...");
                Handled::Ok
            }

            fn on_kill(&mut self) -> Handled {
                info!(self.log(), "Killing...");
                Handled::Ok
            }
        }
    };
}

/// A macro that provides an empty implementation of [ComponentLifecycle](ComponentLifecycle) for the given component
///
/// Use this in components that do not require any special treatment of control events.
///
/// # Example
///
/// To ignore control events for a component `TestComponent`, write:
/// `ignore_control!(TestComponent);`
#[deprecated(
    since = "0.10.0",
    note = "Use the ignore_lifecyle macro instead. This macro will be removed in future version of Kompact"
)]
#[macro_export]
macro_rules! ignore_control {
    ($component:ty) => {
        ignore_lifecycle!($component);
    };
}

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
            fn handle(&mut self, _event: <$port as Port>::Request) -> Handled {
                Handled::Ok // ignore all
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
            fn handle(&mut self, _event: <$port as Port>::Indication) -> Handled {
                Handled::Ok // ignore all
            }
        }
    };
}

#[cfg(not(nightly))]
mod iter_extras {
    use crate::serialisation::{SerError, TryClone};

    /// Additional iterator functions
    pub trait IterExtras: Iterator + Sized {
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
            if let Some(item) = current.take() {
                f(item, t)
            }
        }

        /// Iterate over each item in the iterator and apply a function to it and a clone of the given value `t`
        /// if such a clone can be created
        ///
        /// Behaves like `iterator.for_each(|item| f(item, t.try_clone()))`, except that it avoids cloning
        /// in the case where the iterator contains a single item or for the last item in a larger iterator.
        /// It also shortcircuits on cloning errors.
        ///
        /// Use this when trying to clone `T` is relatively expensive compared to `f`.
        fn for_each_try_with<T, F>(mut self, t: T, mut f: F) -> Result<(), SerError>
        where
            T: TryClone,
            F: FnMut(Self::Item, T),
        {
            let mut current: Option<Self::Item> = self.next();
            let mut next: Option<Self::Item> = self.next();
            while next.is_some() {
                let item = current.take().unwrap();
                let cloned = t.try_clone()?;
                f(item, cloned);
                current = next;
                next = self.next();
            }
            if let Some(item) = current.take() {
                f(item, t)
            }
            Ok(())
        }
    }

    impl<T: Sized> IterExtras for T where T: Iterator {}
}

// this variant requires #![feature(unsized_locals)]
#[cfg(nightly)]
mod iter_extras {
    use crate::serialisation::{SerError, TryClone};

    /// Additional iterator functions
    pub trait IterExtras: Iterator {
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
            if let Some(item) = current.take() {
                f(item, t)
            }
        }

        /// Iterate over each item in the iterator and apply a function to it and a clone of the given value `t`
        /// if such a clone can be created
        ///
        /// Behaves like `iterator.for_each(|item| f(item, t.try_clone()))`, except that it avoids cloning
        /// in the case where the iterator contains a single item or for the last item in a larger iterator.
        /// It also shortcircuits on cloning errors.
        ///
        /// Use this when trying to clone `T` is relatively expensive compared to `f`.
        fn for_each_try_with<T, F>(mut self, t: T, mut f: F) -> Result<(), SerError>
        where
            T: TryClone,
            F: FnMut(Self::Item, T),
        {
            let mut current: Option<Self::Item> = self.next();
            let mut next: Option<Self::Item> = self.next();
            while next.is_some() {
                let item = current.take().unwrap();
                let cloned = t.try_clone()?;
                f(item, cloned);
                current = next;
                next = self.next();
            }
            if let Some(item) = current.take() {
                f(item, t)
            }
            Ok(())
        }
    }

    impl<T: ?Sized> IterExtras for T where T: Iterator {}
}

#[cfg(all(nightly, feature = "type_erasure"))]
pub mod erased {
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
        start_f
            .wait_timeout(WAIT_TIMEOUT)
            .expect("Component start");

        let ask_f = tc_ref.ask(|promise| Ask::new(promise, 42u64));
        ask_f
            .wait_timeout(WAIT_TIMEOUT)
            .expect("Response");

        tc.on_definition(|c| {
            assert_eq!(c.counter, 42u64);
        });

        let ask_f2 = tc_sref.ask(Ask::of(1u64));
        ask_f2
            .wait_timeout(WAIT_TIMEOUT)
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

    #[derive(ComponentDefinition)]
    struct AsyncTestComponent {
        ctx: ComponentContext<Self>,
        proxee: ActorRef<Ask<u64, ()>>,
    }

    impl AsyncTestComponent {
        fn new(proxee: ActorRef<Ask<u64, ()>>) -> AsyncTestComponent {
            AsyncTestComponent {
                ctx: ComponentContext::uninitialised(),
                proxee,
            }
        }
    }

    ignore_lifecycle!(AsyncTestComponent);

    impl Actor for AsyncTestComponent {
        type Message = Ask<u64, ()>;

        fn receive_local(&mut self, msg: Self::Message) -> Handled {
            Handled::block_on(self, async move |async_self| async {
                msg.complete_with(async move |num| async {
                    async_self.proxee.ask(Ask::of(num))
                })
            })
        }

        fn receive_network(&mut self, _msg: NetMessage) -> Handled {
            unimplemented!();
        }
    }

    #[test]
    fn test_ask_complete_with() -> () {
        let system = KompactConfig::default().build().expect("System");
        {
        let tc = system.create(TestComponent::new);
        let tc_ref = tc.actor_ref();
        let atc = system.create(||AsyncTestComponent::new(tc_ref));
        let atc_ref = atc.actor_ref();

        system.start_notify(&tc).wait_timeout(WAIT_TIMEOUT)
            .expect("Component start");
        system.start_notify(&atc).wait_timeout(WAIT_TIMEOUT)
            .expect("Component start");

        let ask_f = atc_ref.ask(|promise| Ask::new(promise, 42u64));
        ask_f
            .wait_timeout(WAIT_TIMEOUT)
            .expect("Response");

        tc.on_definition(|c| {
            assert_eq!(c.counter, 42u64);
        });

        let ask_f2 = atc_ref.ask(Ask::of(1u64));
        ask_f2
            .wait_timeout(WAIT_TIMEOUT)
            .expect("Response2");

        tc.on_definition(|c| {
            assert_eq!(c.counter, 43u64);
        });
}  
        system
            .shutdown()
            .expect("Kompact didn't shut down properly");
    }

    #[cfg(all(nightly, feature = "type_erasure"))]
    #[test]
    fn test_erased_components() {
        use utils::erased::CreateErased;
        let system = KompactConfig::default().build().expect("System");

        {
            let erased_definition: Box<dyn CreateErased<Ask<u64, ()>>> =
                Box::new(TestComponent::new());
            let erased = system.create_erased(erased_definition);
            let actor_ref = erased.actor_ref();

            let start_f = system.start_notify(&erased);
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

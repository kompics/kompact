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

mod ask;
pub use ask::*;
pub(crate) mod checked_casts;
mod iter_extras;
pub use iter_extras::*;
mod macros;
pub use macros::*;
#[cfg(all(nightly, feature = "type_erasure"))]
pub mod erased;

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
#[must_use = "Futures should not simply be dropped, as they usually indicate a delayed operation that must be awaited."]
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

/// Anything that can be completed without providing a value.
pub trait Completable {
    /// Complete self
    ///
    /// Returns a [PromiseErr](PromiseErr) if unsuccessful.
    fn complete(self) -> Result<(), PromiseErr>;
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

impl<T: Send + Sized + Default> Completable for KPromise<T> {
    fn complete(self) -> Result<(), PromiseErr> {
        self.fulfil(Default::default())
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

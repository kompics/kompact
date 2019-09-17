use std::sync::Arc;

use std::{
    ops::DerefMut,
    sync::{mpsc, TryLockError},
    time::Duration,
};

use super::*;

//pub fn connect<P: Port>(c: &ProvidedRef<P>, p: &mut Required<P>) -> () {}

#[derive(Debug)]
pub enum TryDualLockError {
    LeftWouldBlock,
    RightWouldBlock,
    LeftPoisoned,
    RightPoisoned,
}

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

pub fn promise<T: Send + Sized>() -> (Promise<T>, Future<T>) {
    let (tx, rx) = mpsc::channel();
    let f = Future { result_channel: rx };
    let p = Promise { result_channel: tx };
    (p, f)
}

#[derive(Debug)]
pub enum PromiseErr {
    ChannelBroken,
    AlreadyFulfilled,
}

/// Until the futures crate stabilises
#[derive(Debug)]
pub struct Future<T: Send + Sized> {
    result_channel: mpsc::Receiver<T>,
}

impl<T: Send + Sized> Future<T> {
    pub fn wait(self) -> T {
        self.result_channel.recv().unwrap()
    }

    pub fn wait_timeout(self, timeout: Duration) -> Result<T, Future<T>> {
        self.result_channel.recv_timeout(timeout).map_err(|_| self)
    }
}

pub trait Fulfillable<T> {
    fn fulfill(self, t: T) -> Result<(), PromiseErr>;
}

/// Until the futures crate stabilises
#[derive(Debug, Clone)]
pub struct Promise<T: Send + Sized> {
    result_channel: mpsc::Sender<T>,
}

impl<T: Send + Sized> Fulfillable<T> for Promise<T> {
    fn fulfill(self, t: T) -> Result<(), PromiseErr> {
        self.result_channel
            .send(t)
            .map_err(|_| PromiseErr::ChannelBroken)
    }
}

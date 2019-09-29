use std::sync::Arc;

use std::{
    fmt,
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
impl<T: Send + Sized + fmt::Debug, E: Send + Sized + fmt::Debug> Future<Result<T, E>> {
    pub fn wait_expect(self, timeout: Duration, error_msg: &'static str) -> T {
        self.wait_timeout(timeout)
            .expect(&format!("{} (caused by timeout)", error_msg))
            .expect(&format!("{} (caused by result)", error_msg))
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
    pub fn new(promise: Promise<Response>, content: Request) -> Ask<Request, Response> {
        Ask { promise, content }
    }

    pub fn of(content: Request) -> impl FnOnce(Promise<Response>) -> Ask<Request, Response> {
        |promise| Ask::new(promise, content)
    }

    pub fn request(&self) -> &Request {
        &self.content
    }

    pub fn request_mut(&mut self) -> &mut Request {
        &mut self.content
    }

    pub fn take(self) -> (Promise<Response>, Request) {
        (self.promise, self.content)
    }

    pub fn reply(self, response: Response) -> Result<(), PromiseErr> {
        self.promise.fulfill(response)
    }

    pub fn complete<F>(self, f: F) -> Result<(), PromiseErr>
    where
        F: FnOnce(Request) -> Response,
    {
        let response = f(self.content);
        self.promise.fulfill(response)
    }
}

#[macro_export]
macro_rules! ignore_control {
    ($component:ty) => {
        impl Provide<ControlPort> for $component {
            fn handle(&mut self, _event: ControlEvent) -> () {
                () // ignore all
            }
        }
    };
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
}

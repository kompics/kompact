use std::{
    fmt,
    ops::DerefMut,
    panic,
    sync::{atomic::AtomicU64, Arc, Mutex, Weak},
    time::Duration,
};
use uuid::Uuid;
//use oncemutex::OnceMutex;
use hocon::Hocon;
use std::cell::{RefCell, UnsafeCell};

use super::*;
use crate::{
    actors::TypedMsgQueue,
    messaging::{
        DispatchEnvelope,
        MsgEnvelope,
        PathResolvable,
        RegistrationEnvelope,
        RegistrationId,
        RegistrationResponse,
    },
    supervision::*,
    timer::timer_manager::{ExecuteAction, ScheduledTimer, Timer, TimerManager, TimerRefFactory},
};

use crate::net::buffer::EncodeBuffer;
#[cfg(all(nightly, feature = "type_erasure"))]
use crate::utils::erased::CreateErased;
use owning_ref::{Erased, OwningRefMut};
use std::any::Any;

mod component_context;
pub(crate) mod lifecycle;
pub use component_context::*;
mod actual_component;
pub use actual_component::*;
mod system_handle;
use system_handle::*;
mod definition;
pub use definition::*;
mod component_core;
pub use component_core::*;
mod future_task;
use future_task::*;

/// A trait for abstracting over structures that contain a component core
///
/// Used for implementing scheduling and execution logic,
/// such as [Scheduler](runtime::Scheduler).
pub trait CoreContainer: Send + Sync {
    /// Returns the component's unique id
    fn id(&self) -> &Uuid;
    /// Returns a reference to the actual component core
    fn core(&self) -> &ComponentCore;
    /// Executes this component on the current thread
    fn execute(&self) -> SchedulingDecision;
    /// Returns a reference to this component's control port
    fn control_port(&self) -> ProvidedRef<ControlPort>;
    /// Returns this component's system
    fn system(&self) -> &KompactSystem {
        self.core().system()
    }
    /// Schedules this component on its associated scheduler
    fn schedule(&self) -> ();
    /// The descriptive string of the [ComponentDefinition](ComponentDefinition) type wrapped in this container
    fn type_name(&self) -> &'static str;
}

impl fmt::Debug for dyn CoreContainer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CoreContainer({})", self.id())
    }
}

pub(crate) struct ComponentMutableCore<C> {
    pub(crate) definition: C,
    skip: usize,
}

/// Statistics about the last invocation of [execute](ComponentDefinition::execute).
pub struct ExecuteResult {
    blocking: bool,
    count: usize,
    skip: usize,
}

impl ExecuteResult {
    /// Create a new execute result
    ///
    /// `count` gives the total number of events handled during the invocation
    /// `skip` gives the offset from where queues will be checked during the next invocation (used for fairness)
    pub fn new(blocking: bool, count: usize, skip: usize) -> ExecuteResult {
        ExecuteResult {
            blocking,
            count,
            skip,
        }
    }
}

/// An abstraction over providers of Kompact loggers
pub trait ComponentLogging {
    /// Returns a reference to the component's logger instance
    ///
    /// See [log](ComponentContext::log) for more details.
    fn log(&self) -> &KompactLogger;
}

impl<CD> ComponentLogging for CD
where
    CD: ComponentDefinition + 'static,
{
    fn log(&self) -> &KompactLogger {
        self.ctx().log()
    }
}

/// A trait implementing handling of provided events of `P`
///
/// This is equivalent to a Kompics *Handler* subscribed on a provided port of type `P`.
pub trait Provide<P: Port + 'static> {
    /// Handle the port's `event`
    ///
    /// # Note
    ///
    /// Remember that components usually run on a shared thread pool,
    /// so you shouldn't ever block in this method unless you know what you are doing.
    fn handle(&mut self, event: P::Request) -> ();
}

/// A trait implementing handling of required events of `P`
///
/// This is equivalent to a Kompics *Handler* subscribed on a required port of type `P`.
pub trait Require<P: Port + 'static> {
    /// Handle the port's `event`
    ///
    /// # Note
    ///
    /// Remember that components usually run on a shared thread pool,
    /// so you shouldn't ever block in this method unless you know what you are doing.
    fn handle(&mut self, event: P::Indication) -> ();
}

/// A convenience abstraction over concrete port instance fields
///
/// This trait is usually automatically derived when using `#[derive(ComponentDefinition)]`.
pub trait ProvideRef<P: Port + 'static> {
    /// Returns a provided reference to this component's port instance of type `P`
    fn provided_ref(&mut self) -> ProvidedRef<P>;
    /// Connects this component's provided port instance of type `P` to `req`
    fn connect_to_required(&mut self, req: RequiredRef<P>) -> ();
}

/// A convenience abstraction over concrete port instance fields
///
/// This trait is usually automatically derived when using `#[derive(ComponentDefinition)]`.
pub trait RequireRef<P: Port + 'static> {
    /// Returns a required reference to this component's port instance of type `P`
    fn required_ref(&mut self) -> RequiredRef<P>;
    /// Connects this component's required port instance of type `P` to `prov`
    fn connect_to_provided(&mut self, prov: ProvidedRef<P>) -> ();
}

/// Same as [ProvideRef](ProvideRef), but for instances that must be locked first
///
/// This is used, for example, with an `Arc<Component<_>>`.
pub trait LockingProvideRef<P: Port + 'static> {
    /// Returns a required reference to this component's port instance of type `P`
    fn provided_ref(&self) -> ProvidedRef<P>;
    /// Connects this component's required port instance of type `P` to `prov`
    fn connect_to_required(&self, req: RequiredRef<P>) -> ();
}

/// Same as [RequireRef](RequireRef), but for instances that must be locked first
///
/// This is used, for example, with an `Arc<Component<_>>`.
pub trait LockingRequireRef<P: Port + 'static> {
    /// Returns a required reference to this component's port instance of type `P`
    fn required_ref(&self) -> RequiredRef<P>;
    /// Connects this component's required port instance of type `P` to `prov`
    fn connect_to_provided(&self, prov: ProvidedRef<P>) -> ();
}

impl<P, CD> LockingProvideRef<P> for Arc<Component<CD>>
where
    P: Port + 'static,
    CD: ComponentDefinition + Provide<P> + ProvideRef<P>,
{
    fn provided_ref(&self) -> ProvidedRef<P> {
        self.on_definition(|cd| ProvideRef::provided_ref(cd))
    }

    fn connect_to_required(&self, req: RequiredRef<P>) -> () {
        self.on_definition(|cd| ProvideRef::connect_to_required(cd, req))
    }
}

impl<P, CD> LockingRequireRef<P> for Arc<Component<CD>>
where
    P: Port + 'static,
    CD: ComponentDefinition + Require<P> + RequireRef<P>,
{
    fn required_ref(&self) -> RequiredRef<P> {
        self.on_definition(|cd| RequireRef::required_ref(cd))
    }

    fn connect_to_provided(&self, prov: ProvidedRef<P>) -> () {
        self.on_definition(|cd| RequireRef::connect_to_provided(cd, prov))
    }
}

/// Indicates whether or not a component should be sent to the [Scheduler](runtime::Scheduler)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedulingDecision {
    /// Sent the component to the [Scheduler](runtime::Scheduler)
    ///
    /// That is, call [schedule](CoreContainer::schedule).
    Schedule,
    /// Don't schedule the component, because it is already scheduled
    AlreadyScheduled,
    /// Don't schedule the component, because it has nothing to do
    NoWork,
    /// Don't schedule the component, because it is blocked
    Blocked,
    /// Immediately execute the component again, as it just came out of blocking
    Resume,
}

impl SchedulingDecision {
    pub fn or_use(self, other: impl Fn() -> SchedulingDecision) -> SchedulingDecision {
        match self {
            SchedulingDecision::Schedule
            | SchedulingDecision::AlreadyScheduled
            | SchedulingDecision::Blocked
            | SchedulingDecision::Resume => self,
            SchedulingDecision::NoWork => match other() {
                SchedulingDecision::Schedule
                | SchedulingDecision::Resume
                | SchedulingDecision::AlreadyScheduled => SchedulingDecision::Resume,
                x => x,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{component::AbstractComponent, prelude::*};
    use futures::channel::oneshot;
    use std::{sync::Arc, thread, time::Duration};

    #[derive(ComponentDefinition, Actor)]
    struct TestComponent {
        ctx: ComponentContext<TestComponent>,
    }

    impl TestComponent {
        fn new() -> TestComponent {
            TestComponent {
                ctx: ComponentContext::new(),
            }
        }
    }

    impl Provide<ControlPort> for TestComponent {
        fn handle(&mut self, event: ControlEvent) -> () {
            if let ControlEvent::Start = event {
                info!(self.ctx.log(), "Starting TestComponent");
            }
        }
    }

    #[test]
    fn component_core_send() -> () {
        let system = KompactConfig::default().build().expect("KompactSystem");
        let cc = system.create(TestComponent::new);
        let core = cc.core();
        is_send(&core.id);
        is_send(&core.system);
        is_send(&core.state);
        // component is clearly not Send, but that's ok
        is_sync(&core.id);
        is_sync(&core.system);
        is_sync(&core.state);
        // component is clearly not Sync, but that's ok
    }

    // Just a way to force the compiler to infer Send for T
    fn is_send<T: Send>(_v: &T) -> () {
        // ignore
    }

    // Just a way to force the compiler to infer Sync for T
    fn is_sync<T: Sync>(_v: &T) -> () {
        // ignore
    }

    #[derive(Debug, Copy, Clone)]
    struct TestMessage;
    impl Serialisable for TestMessage {
        fn ser_id(&self) -> SerId {
            Self::SER_ID
        }

        fn size_hint(&self) -> Option<usize> {
            Some(0)
        }

        fn serialise(&self, _buf: &mut dyn BufMut) -> Result<(), SerError> {
            Ok(())
        }

        fn local(self: Box<Self>) -> Result<Box<dyn Any + Send>, Box<dyn Serialisable>> {
            Ok(self)
        }
    }
    impl Deserialiser<TestMessage> for TestMessage {
        // whatever
        const SER_ID: SerId = 42;

        fn deserialise(_buf: &mut dyn Buf) -> Result<TestMessage, SerError> {
            Ok(TestMessage)
        }
    }

    #[derive(ComponentDefinition)]
    struct ChildComponent {
        ctx: ComponentContext<Self>,
        got_message: bool,
    }
    impl ChildComponent {
        fn new() -> Self {
            ChildComponent {
                ctx: ComponentContext::new(),
                got_message: false,
            }
        }
    }
    ignore_control!(ChildComponent);
    impl NetworkActor for ChildComponent {
        type Deserialiser = TestMessage;
        type Message = TestMessage;

        fn receive(&mut self, _sender: Option<ActorPath>, _msg: Self::Message) -> () {
            info!(self.log(), "Child got message");
            self.got_message = true;
        }
    }

    #[derive(Debug)]
    enum ParentMessage {
        GetChild(KPromise<Arc<Component<ChildComponent>>>),
        RegResp(RegistrationResponse),
    }

    impl From<RegistrationResponse> for ParentMessage {
        fn from(rr: RegistrationResponse) -> Self {
            ParentMessage::RegResp(rr)
        }
    }

    #[derive(ComponentDefinition)]
    struct ParentComponent {
        ctx: ComponentContext<Self>,
        alias_opt: Option<String>,
        child: Option<Arc<Component<ChildComponent>>>,
        reg_id: Option<RegistrationId>,
    }
    impl ParentComponent {
        fn unique() -> Self {
            ParentComponent {
                ctx: ComponentContext::new(),
                alias_opt: None,
                child: None,
                reg_id: None,
            }
        }

        fn alias(s: String) -> Self {
            ParentComponent {
                ctx: ComponentContext::new(),
                alias_opt: Some(s),
                child: None,
                reg_id: None,
            }
        }
    }
    impl Provide<ControlPort> for ParentComponent {
        fn handle(&mut self, event: ControlEvent) -> () {
            match event {
                ControlEvent::Start => {
                    let child = self.ctx.system().create(ChildComponent::new);
                    let id = match self.alias_opt.take() {
                        Some(s) => self.ctx.system().register_by_alias(&child, s, self),
                        None => self.ctx.system().register(&child, self),
                    };
                    self.reg_id = Some(id);
                    self.child = Some(child);
                }
                ControlEvent::Stop | ControlEvent::Kill => {
                    let _ = self.child.take(); // don't hang on to the child
                }
            }
        }
    }
    impl Actor for ParentComponent {
        type Message = ParentMessage;

        fn receive_local(&mut self, msg: Self::Message) -> () {
            match msg {
                ParentMessage::GetChild(promise) => {
                    if let Some(ref child) = self.child {
                        promise.fulfil(child.clone()).expect("fulfilled");
                    } else {
                        drop(promise); // this will cause an error on the Future side
                    }
                }
                ParentMessage::RegResp(res) => {
                    assert_eq!(res.id, self.reg_id.take().unwrap());
                    info!(self.log(), "Child was registered");
                    if let Some(ref child) = self.child {
                        self.ctx.system().start(child);
                        let path = res.result.expect("actor path");
                        path.tell(TestMessage, self);
                    } else {
                        unreachable!();
                    }
                }
            }
        }

        fn receive_network(&mut self, _msg: NetMessage) -> () {
            unimplemented!("Shouldn't be used");
        }
    }

    #[test]
    fn child_unique_registration_test() -> () {
        let wait_time = Duration::from_millis(1000);

        let mut conf = KompactConfig::default();
        conf.system_components(DeadletterBox::new, NetworkConfig::default().build());
        let system = conf.build().expect("system");
        let parent = system.create(ParentComponent::unique);
        system.start(&parent);
        thread::sleep(wait_time);
        let (p, f) = kpromise::<Arc<Component<ChildComponent>>>();
        parent.actor_ref().tell(ParentMessage::GetChild(p));
        let child = f.wait_timeout(wait_time).expect("child");
        let stop_f = system.stop_notify(&child);
        system.stop(&parent);

        stop_f.wait_timeout(wait_time).expect("child didn't stop");
        child.on_definition(|cd| {
            assert!(cd.got_message, "child didn't get the message");
        });
        system.shutdown().expect("shutdown");
    }

    const TEST_ALIAS: &str = "test";

    #[test]
    fn child_alias_registration_test() -> () {
        let wait_time = Duration::from_millis(1000);

        let mut conf = KompactConfig::default();
        conf.system_components(DeadletterBox::new, NetworkConfig::default().build());
        let system = conf.build().expect("system");
        let parent = system.create(|| ParentComponent::alias(TEST_ALIAS.into()));
        system.start(&parent);
        thread::sleep(wait_time);
        let (p, f) = kpromise::<Arc<Component<ChildComponent>>>();
        parent.actor_ref().tell(ParentMessage::GetChild(p));
        let child = f.wait_timeout(wait_time).expect("child");
        let stop_f = system.stop_notify(&child);
        system.stop(&parent);

        stop_f.wait_timeout(wait_time).expect("child didn't stop");
        child.on_definition(|cd| {
            assert!(cd.got_message, "child didn't get the message");
        });
        system.shutdown().expect("shutdown");
    }

    #[test]
    fn test_dynamic_port_access() -> () {
        struct A;
        impl Port for A {
            type Indication = u64;
            type Request = String;
        }
        struct B;
        impl Port for B {
            type Indication = &'static str;
            type Request = i8;
        }

        #[derive(ComponentDefinition, Actor)]
        struct TestComp {
            ctx: ComponentContext<Self>,
            req_a: RequiredPort<A>,
            prov_b: ProvidedPort<B>,
        }

        impl TestComp {
            fn new() -> TestComp {
                TestComp {
                    ctx: ComponentContext::new(),
                    req_a: RequiredPort::new(),
                    prov_b: ProvidedPort::new(),
                }
            }
        }
        ignore_control!(TestComp);
        ignore_requests!(B, TestComp);
        ignore_indications!(A, TestComp);

        let system = KompactConfig::default().build().expect("System");
        let comp = system.create(TestComp::new);
        let dynamic: Arc<dyn AbstractComponent<Message = Never>> = comp;
        dynamic.on_dyn_definition(|def| {
            assert!(def.get_required_port::<A>().is_some());
            assert!(def.get_provided_port::<A>().is_none());

            assert!(def.get_required_port::<B>().is_none());
            assert!(def.get_provided_port::<B>().is_some());
        });

        system.shutdown().expect("shutdown");
    }

    #[derive(Debug)]
    enum BlockMe {
        Now,
        OnChannel(oneshot::Receiver<String>),
    }

    #[derive(ComponentDefinition)]
    struct BlockingComponent {
        ctx: ComponentContext<Self>,
        test_string: String,
    }

    impl BlockingComponent {
        fn new() -> Self {
            BlockingComponent {
                ctx: ComponentContext::new(),
                test_string: "started".to_string(),
            }
        }
    }

    impl Provide<ControlPort> for BlockingComponent {
        fn handle(&mut self, event: ControlEvent) -> () {
            if let ControlEvent::Start = event {
                info!(self.log(), "Starting BlockingComponent");
            }
        }
    }

    impl Actor for BlockingComponent {
        type Message = BlockMe;

        fn receive_local(&mut self, msg: Self::Message) {
            match msg {
                BlockMe::Now => {
                	info!(self.log(), "Got BlockMe::Now");
                    self.block_on(async move |mut async_self| {
                        async_self.test_string = "done".to_string();
                        info!(async_self.log(), "Ran BlockMe::Now future");
                    });
                }
                BlockMe::OnChannel(receiver) => {
                	info!(self.log(), "Got BlockMe::OnChannel");
                    self.block_on(async move |mut async_self| {
                    	info!(async_self.log(), "Started BlockMe::OnChannel future");
                        let s = receiver.await;
                        async_self.test_string = s.expect("Some string");
                        info!(async_self.log(), "Completed BlockMe::OnChannel future");
                    });
                }
            }
        }

        fn receive_network(&mut self, _msg: NetMessage) {
            unimplemented!("No networking here!");
        }
    }

    #[test]
    fn test_immediate_blocking() {
        let timeout = Duration::from_millis(1000);
        let system = KompactConfig::default().build().expect("System");
        let comp = system.create(BlockingComponent::new);
        system
            .start_notify(&comp)
            .wait_timeout(timeout)
            .expect("Component didn't start");
        comp.actor_ref().tell(BlockMe::Now);
        thread::sleep(timeout);
        system
            .kill_notify(comp.clone())
            .wait_timeout(timeout)
            .expect("Component didn't die");
        comp.on_definition(|cd| {
            assert_eq!(cd.test_string, "done");
        });
        system.shutdown().expect("shutdown");
    }

    #[test]
    fn test_channel_blocking() {
        let timeout = Duration::from_millis(1000);
        let system = KompactConfig::default().build().expect("System");
        let comp = system.create(BlockingComponent::new);
        system
            .start_notify(&comp)
            .wait_timeout(timeout)
            .expect("Component didn't start");

        let (sender, receiver) = oneshot::channel();
        comp.actor_ref().tell(BlockMe::OnChannel(receiver));
        thread::sleep(timeout);
        sender.send("gotcha".to_string()).expect("Should have sent");
        thread::sleep(timeout);
        system
            .kill_notify(comp.clone())
            .wait_timeout(timeout)
            .expect("Component didn't die");
        comp.on_definition(|cd| {
            assert_eq!(cd.test_string, "gotcha");
        });
        system.shutdown().expect("shutdown");
    }

    #[test]
    fn test_mixed_blocking() {
        let timeout = Duration::from_millis(1000);
        let system = KompactConfig::default().build().expect("System");
        let comp = system.create(BlockingComponent::new);
        system
            .start_notify(&comp)
            .wait_timeout(timeout)
            .expect("Component didn't start");

        let (sender, receiver) = oneshot::channel();
        comp.actor_ref().tell(BlockMe::OnChannel(receiver));
        thread::sleep(timeout);
        comp.actor_ref().tell(BlockMe::Now);
        sender.send("gotcha".to_string()).expect("Should have sent");
        thread::sleep(timeout);
        system
            .kill_notify(comp.clone())
            .wait_timeout(timeout)
            .expect("Component didn't die");
        comp.on_definition(|cd| {
            assert_eq!(cd.test_string, "done");
        });
        system.shutdown().expect("shutdown");
    }
}

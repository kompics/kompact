use hocon::Hocon;
use std::{
    cell::{RefCell, UnsafeCell},
    fmt,
    ops::DerefMut,
    panic,
    sync::{atomic::AtomicU64, Arc, Mutex, Weak},
    time::Duration,
};
use uuid::Uuid;

use super::*;
use crate::{
    actors::TypedMsgQueue,
    net::buffers::EncodeBuffer,
    supervision::*,
    timer::timer_manager::{ExecuteAction, ScheduledTimer, Timer, TimerManager, TimerRefFactory},
};
use rustc_hash::FxHashMap;

#[cfg(all(nightly, feature = "type_erasure"))]
use crate::utils::erased::CreateErased;
use owning_ref::{Erased, OwningRefMut};
use std::any::Any;

mod context;
pub use context::*;
mod actual_component;
pub(crate) mod lifecycle;
pub use actual_component::*;
mod system_handle;
pub use system_handle::*;
mod definition;
pub use definition::*;
mod core;
pub use self::core::*;
mod future_task;
pub use future_task::*;

/// State transition indication at the end of a message or event handler
#[must_use = "The Handled value must be returned from a handle or receive function in order to take effect."]
#[derive(Debug)]
pub enum Handled {
    /// Continue as normal
    Ok,
    /// Immediately suspend processing of any messages and events
    /// until the `BlockingFuture` has completed
    BlockOn(BlockingFuture),
    /// Kill the component without handling any further messages
    DieNow,
}
impl Handled {
    /// Constructs a state transition instruction which causes
    /// the component to suspend processing of any messages and events
    /// until the async `fun` (the returned [Future](std::future::Future)) has completed.
    ///
    /// Mutable access to the component's internal state is provided via
    /// the [ComponentDefinitionAccess](ComponentDefinitionAccess) guard object.
    ///
    /// Please see the documentation for [ComponentDefinitionAccess](ComponentDefinitionAccess)
    /// for details on how the internal state may (and may not) be used.
    ///
    /// # Example
    ///
    /// ```
    /// # use kompact::prelude::*;
    ///
    /// #[derive(ComponentDefinition, Actor)]
    /// struct AsyncComponent {
    ///    ctx: ComponentContext<Self>,
    ///    flag: bool,
    /// }
    /// impl AsyncComponent {
    ///     fn new() -> Self {
    ///         AsyncComponent {
    ///             ctx: ComponentContext::uninitialised(),
    ///             flag: false,    
    ///         }
    ///     }   
    /// }
    /// impl ComponentLifecycle for AsyncComponent {
    ///     fn on_start(&mut self) -> Handled {
    ///         // on nightly you can just write: async move |mut async_self| {...}
    ///         Handled::block_on(self, move |mut async_self| async move {
    ///             async_self.flag = true;
    ///             Handled::Ok
    ///         })
    ///     }   
    /// }
    /// ```
    ///
    /// # See Also
    ///
    /// In order to continue processing messages and events in parallel to completing the future
    /// use [spawn_local](ComponentDefinition::spawn_local).
    ///
    /// In order to run a large future which does not need access to component's internal state
    /// at all or until the very end, consider using [spawn_off](ComponentDefinition::spawn_off).
    pub fn block_on<CD, F>(
        component: &mut CD,
        fun: impl FnOnce(ComponentDefinitionAccess<CD>) -> F,
    ) -> Self
    where
        CD: ComponentDefinition + 'static,
        F: std::future::Future + Send + 'static,
    {
        let blocking = future_task::blocking(component, fun);
        Handled::BlockOn(blocking)
    }

    /// Returns true if this instance is an [Handled::Ok](Handled::Ok) variant
    pub fn is_ok(&self) -> bool {
        matches!(self, Handled::Ok)
    }
}
impl Default for Handled {
    fn default() -> Self {
        Handled::Ok
    }
}

/// A trait bound alias for the trait required by the
/// generic parameter of a [Component](Component)
pub trait ComponentTraits: ComponentDefinition + ActorRaw + Sized + 'static {}
impl<CD> ComponentTraits for CD where CD: ComponentDefinition + ActorRaw + Sized + 'static {}

/// A trait for abstracting over structures that contain a component core
///
/// Used for implementing scheduling and execution logic,
/// such as [Scheduler](runtime::Scheduler).
pub trait CoreContainer: Send + Sync {
    /// Returns the component's unique id
    fn id(&self) -> Uuid;
    /// Returns a reference to the actual component core
    fn core(&self) -> &ComponentCore;
    /// Executes this component on the current thread
    fn execute(&self) -> SchedulingDecision;
    /// Returns this component's system
    fn system(&self) -> &KompactSystem {
        self.core().system()
    }
    /// Schedules this component on its associated scheduler
    fn schedule(&self) -> ();
    /// The descriptive string of the [ComponentDefinition](ComponentDefinition) type wrapped in this container
    fn type_name(&self) -> &'static str;

    /// Returns the underlying message queue of this component
    /// without the type information
    fn dyn_message_queue(&self) -> &dyn DynMsgQueue;

    /// Enqueue an event on the component's control queue
    ///
    /// Not usually something you need to manually,
    /// unless you nned custom supervisor behaviours, for example.
    fn enqueue_control(&self, event: ControlEvent) -> ();
}

impl fmt::Debug for dyn CoreContainer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CoreContainer({})", self.id())
    }
}

/// A trait for component views that can be used for unique actor registration
pub trait UniqueRegistrable: DynActorRefFactory {
    /// Returns the unique id of a component
    fn component_id(&self) -> Uuid;
}

/// Anything with this trait can be turned into an [ActorRef](ActorRef)
/// as long as its behind an Arc or Weak
pub trait MsgQueueContainer: CoreContainer {
    /// The message type of the queue
    type Message: MessageBounds;
    /// The actual underlying queue
    fn message_queue(&self) -> &TypedMsgQueue<Self::Message>;
    /// A weak reference to the component that must be scheduled
    /// when something is enqueued
    fn downgrade_dyn(self: Arc<Self>) -> Weak<dyn CoreContainer>;
}

pub(crate) struct FakeCoreContainer;
impl CoreContainer for FakeCoreContainer {
    fn id(&self) -> Uuid {
        unreachable!("FakeCoreContainer should only be used as a Sized type for `Weak::new()`!");
    }

    fn core(&self) -> &ComponentCore {
        unreachable!("FakeCoreContainer should only be used as a Sized type for `Weak::new()`!");
    }

    fn execute(&self) -> SchedulingDecision {
        unreachable!("FakeCoreContainer should only be used as a Sized type for `Weak::new()`!");
    }

    fn system(&self) -> &KompactSystem {
        unreachable!("FakeCoreContainer should only be used as a Sized type for `Weak::new()`!");
    }

    fn schedule(&self) -> () {
        unreachable!("FakeCoreContainer should only be used as a Sized type for `Weak::new()`!");
    }

    fn type_name(&self) -> &'static str {
        unreachable!("FakeCoreContainer should only be used as a Sized type for `Weak::new()`!");
    }

    fn dyn_message_queue(&self) -> &dyn DynMsgQueue {
        unreachable!("FakeCoreContainer should only be used as a Sized type for `Weak::new()`!");
    }

    fn enqueue_control(&self, _event: ControlEvent) -> () {
        unreachable!("FakeCoreContainer should only be used as a Sized type for `Weak::new()`!");
    }
}

pub(crate) struct ComponentMutableCore<C> {
    pub(crate) definition: C,
    skip: usize,
}
impl<C> ComponentMutableCore<C> {
    fn from(definition: C) -> Self {
        ComponentMutableCore {
            definition,
            skip: 0,
        }
    }
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
    CD: ComponentTraits + ComponentLifecycle,
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
    fn handle(&mut self, event: P::Request) -> Handled;
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
    fn handle(&mut self, event: P::Indication) -> Handled;
}

/// A convenience abstraction over concrete port instance fields
///
/// This trait is usually automatically derived when using `#[derive(ComponentDefinition)]`.
pub trait ProvideRef<P: Port + 'static> {
    /// Returns a provided reference to this component's port instance of type `P`
    fn provided_ref(&mut self) -> ProvidedRef<P>;

    /// Connects this component's provided port instance of type `P` to `req`
    fn connect_to_required(&mut self, req: RequiredRef<P>) -> ();

    /// Disconnects this component's provided port instance of type `P` from `req`
    fn disconnect(&mut self, req: RequiredRef<P>) -> ();
}

/// A convenience abstraction over concrete port instance fields
///
/// This trait is usually automatically derived when using `#[derive(ComponentDefinition)]`.
pub trait RequireRef<P: Port + 'static> {
    /// Returns a required reference to this component's port instance of type `P`
    fn required_ref(&mut self) -> RequiredRef<P>;

    /// Connects this component's required port instance of type `P` to `prov`
    fn connect_to_provided(&mut self, prov: ProvidedRef<P>) -> ();

    /// Disconnects this component's required port instance of type `P` from `prov`
    fn disconnect(&mut self, prov: ProvidedRef<P>) -> ();
}

/// Same as [ProvideRef](ProvideRef), but for instances that must be locked first
///
/// This is used, for example, with an `Arc<Component<_>>`.
pub trait LockingProvideRef<P, C>
where
    P: Port + 'static,
    C: ComponentDefinition + Sized + 'static + Provide<P> + ProvideRef<P>,
{
    /// Returns a required reference to this component's port instance of type `P`
    fn provided_ref(&self) -> ProvidedRef<P>;

    /// Connects this component's required port instance of type `P` to `prov`
    fn connect_to_required(&self, req: RequiredRef<P>) -> ProviderChannel<P, C>;
}

/// Same as [RequireRef](RequireRef), but for instances that must be locked first
///
/// This is used, for example, with an `Arc<Component<_>>`.
pub trait LockingRequireRef<P, C>
where
    P: Port + 'static,
    C: ComponentDefinition + Sized + 'static + Require<P> + RequireRef<P>,
{
    /// Returns a required reference to this component's port instance of type `P`
    fn required_ref(&self) -> RequiredRef<P>;

    /// Connects this component's required port instance of type `P` to `prov`
    fn connect_to_provided(&self, prov: ProvidedRef<P>) -> RequirerChannel<P, C>;
}

impl<P, CD> LockingProvideRef<P, CD> for Arc<Component<CD>>
where
    P: Port + 'static,
    CD: ComponentTraits + ComponentLifecycle + Provide<P> + ProvideRef<P>,
{
    fn provided_ref(&self) -> ProvidedRef<P> {
        self.on_definition(|cd| ProvideRef::provided_ref(cd))
    }

    fn connect_to_required(&self, req: RequiredRef<P>) -> ProviderChannel<P, CD> {
        self.on_definition(|cd| ProvideRef::connect_to_required(cd, req.clone()));
        ProviderChannel::new(self, req)
    }
}

impl<P, CD> LockingRequireRef<P, CD> for Arc<Component<CD>>
where
    P: Port + 'static,
    CD: ComponentTraits + ComponentLifecycle + Require<P> + RequireRef<P>,
{
    fn required_ref(&self) -> RequiredRef<P> {
        self.on_definition(|cd| RequireRef::required_ref(cd))
    }

    fn connect_to_provided(&self, prov: ProvidedRef<P>) -> RequirerChannel<P, CD> {
        self.on_definition(|cd| RequireRef::connect_to_provided(cd, prov.clone()));
        RequirerChannel::new(self, prov)
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
    /// Use the current `SchedulingDecision` if it makes a strong decision
    /// or the one returned by given function if the current one is `NoWork`.
    ///
    /// In particular this is used to come out of blocking, where the combination
    /// of `NoWork` and anything that that indicates work (e.g., `AlreadyScheduled`)
    /// actually indicates that we want the component to `Resume` immediately.
    pub fn or_use(self, other: SchedulingDecision) -> SchedulingDecision {
        match self {
            SchedulingDecision::Schedule
            | SchedulingDecision::AlreadyScheduled
            | SchedulingDecision::Blocked
            | SchedulingDecision::Resume => self,
            SchedulingDecision::NoWork => match other {
                SchedulingDecision::Schedule
                | SchedulingDecision::Resume
                | SchedulingDecision::AlreadyScheduled => SchedulingDecision::Resume,
                x => x,
            },
        }
    }

    /// Use the current `SchedulingDecision` if it makes a strong decision
    /// or the one returned by given function if the current one is `NoWork`.
    ///
    /// In particular this is used to come out of blocking, where the combination
    /// of `NoWork` and anything that that indicates work (e.g., `AlreadyScheduled`)
    /// actually indicates that we want the component to `Resume` immediately.
    pub fn or_from(self, other: impl Fn() -> SchedulingDecision) -> SchedulingDecision {
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

    use std::ops::Deref;

    const TIMEOUT: Duration = Duration::from_millis(3000);

    #[derive(ComponentDefinition, Actor)]
    struct TestComponent {
        ctx: ComponentContext<TestComponent>,
    }

    impl TestComponent {
        fn new() -> TestComponent {
            TestComponent {
                ctx: ComponentContext::uninitialised(),
            }
        }
    }

    ignore_lifecycle!(TestComponent);

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
                ctx: ComponentContext::uninitialised(),
                got_message: false,
            }
        }
    }
    ignore_lifecycle!(ChildComponent);
    impl NetworkActor for ChildComponent {
        type Deserialiser = TestMessage;
        type Message = TestMessage;

        fn receive(&mut self, _sender: Option<ActorPath>, _msg: Self::Message) -> Handled {
            info!(self.log(), "Child got message");
            self.got_message = true;
            Handled::Ok
        }
    }

    #[derive(Debug)]
    enum ParentMessage {
        GetChild(KPromise<Arc<Component<ChildComponent>>>),
    }

    #[derive(ComponentDefinition)]
    struct ParentComponent {
        ctx: ComponentContext<Self>,
        alias_opt: Option<String>,
        child: Option<Arc<Component<ChildComponent>>>,
    }
    impl ParentComponent {
        fn unique() -> Self {
            ParentComponent {
                ctx: ComponentContext::uninitialised(),
                alias_opt: None,
                child: None,
            }
        }

        fn alias(s: String) -> Self {
            ParentComponent {
                ctx: ComponentContext::uninitialised(),
                alias_opt: Some(s),
                child: None,
            }
        }
    }

    impl ComponentLifecycle for ParentComponent {
        fn on_start(&mut self) -> Handled {
            let child = self.ctx.system().create(ChildComponent::new);
            let f = match self.alias_opt.take() {
                Some(s) => self.ctx.system().register_by_alias(&child, s),
                None => self.ctx.system().register(&child),
            };
            self.child = Some(child);
            // async move closure syntax is nightly only
            Handled::block_on(self, move |async_self| async move {
                let path = f.await.expect("actor path").expect("actor path");
                info!(async_self.log(), "Child was registered");
                if let Some(ref child) = async_self.child {
                    async_self.ctx.system().start(child);
                    path.tell(TestMessage, async_self.deref());
                } else {
                    unreachable!();
                }
            })
        }

        fn on_stop(&mut self) -> Handled {
            let _ = self.child.take(); // don't hang on to the child
            Handled::Ok
        }

        fn on_kill(&mut self) -> Handled {
            let _ = self.child.take(); // don't hang on to the child
            Handled::Ok
        }
    }
    impl Actor for ParentComponent {
        type Message = ParentMessage;

        fn receive_local(&mut self, msg: Self::Message) -> Handled {
            match msg {
                ParentMessage::GetChild(promise) => {
                    if let Some(ref child) = self.child {
                        promise.fulfil(child.clone()).expect("fulfilled");
                    } else {
                        drop(promise); // this will cause an error on the Future side
                    }
                }
            }
            Handled::Ok
        }

        fn receive_network(&mut self, _msg: NetMessage) -> Handled {
            unimplemented!("Shouldn't be used");
        }
    }

    #[test]
    fn child_unique_registration_test() -> () {
        let mut conf = KompactConfig::default();
        conf.system_components(DeadletterBox::new, NetworkConfig::default().build());
        let system = conf.build().expect("system");
        let parent = system.create(ParentComponent::unique);
        system.start(&parent);
        thread::sleep(TIMEOUT);
        let (p, f) = promise::<Arc<Component<ChildComponent>>>();
        parent.actor_ref().tell(ParentMessage::GetChild(p));
        let child = f.wait_timeout(TIMEOUT).expect("child");
        let stop_f = system.stop_notify(&child);
        system.stop(&parent);

        stop_f.wait_timeout(TIMEOUT).expect("child didn't stop");
        child.on_definition(|cd| {
            assert!(cd.got_message, "child didn't get the message");
        });
        system.shutdown().expect("shutdown");
    }

    const TEST_ALIAS: &str = "test";

    #[test]
    fn child_alias_registration_test() -> () {
        let mut conf = KompactConfig::default();
        conf.system_components(DeadletterBox::new, NetworkConfig::default().build());
        let system = conf.build().expect("system");
        let parent = system.create(|| ParentComponent::alias(TEST_ALIAS.into()));
        system.start(&parent);
        thread::sleep(TIMEOUT);
        let (p, f) = promise::<Arc<Component<ChildComponent>>>();
        parent.actor_ref().tell(ParentMessage::GetChild(p));
        let child = f.wait_timeout(TIMEOUT).expect("child");
        let stop_f = system.stop_notify(&child);
        system.stop(&parent);

        stop_f.wait_timeout(TIMEOUT).expect("child didn't stop");
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
                    ctx: ComponentContext::uninitialised(),
                    req_a: RequiredPort::uninitialised(),
                    prov_b: ProvidedPort::uninitialised(),
                }
            }
        }
        ignore_lifecycle!(TestComp);
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
        SpawnOff(String),
        OnShutdown,
    }

    #[derive(ComponentDefinition)]
    struct BlockingComponent {
        ctx: ComponentContext<Self>,
        test_string: String,
        block_on_shutdown: bool,
    }

    impl BlockingComponent {
        fn new() -> Self {
            BlockingComponent {
                ctx: ComponentContext::uninitialised(),
                test_string: "started".to_string(),
                block_on_shutdown: false,
            }
        }
    }

    impl ComponentLifecycle for BlockingComponent {
        fn on_kill(&mut self) -> Handled {
            if self.block_on_shutdown {
                info!(self.log(), "Cleaning up before shutdown");
                Handled::block_on(self, move |mut async_self| async move {
                    async_self.test_string = "done".to_string();
                    info!(async_self.log(), "Ran BlockMe::OnShutdown future");
                })
            } else {
                Handled::Ok
            }
        }
    }

    impl Actor for BlockingComponent {
        type Message = BlockMe;

        fn receive_local(&mut self, msg: Self::Message) -> Handled {
            match msg {
                BlockMe::Now => {
                    info!(self.log(), "Got BlockMe::Now");
                    // async move closure syntax is nightly only
                    Handled::block_on(self, move |mut async_self| async move {
                        async_self.test_string = "done".to_string();
                        info!(async_self.log(), "Ran BlockMe::Now future");
                    })
                }
                BlockMe::OnChannel(receiver) => {
                    info!(self.log(), "Got BlockMe::OnChannel");
                    // async move closure syntax is nightly only
                    Handled::block_on(self, move |mut async_self| async move {
                        info!(async_self.log(), "Started BlockMe::OnChannel future");
                        let s = receiver.await;
                        async_self.test_string = s.expect("Some string");
                        info!(async_self.log(), "Completed BlockMe::OnChannel future");
                    })
                }
                BlockMe::SpawnOff(s) => {
                    let handle = self.spawn_off(async move { s });
                    // async move closure syntax is nightly only
                    Handled::block_on(self, move |mut async_self| async move {
                        let res = handle.await.expect("result");
                        async_self.test_string = res;
                    })
                }
                BlockMe::OnShutdown => {
                    self.block_on_shutdown = true;
                    Handled::Ok
                }
            }
        }

        fn receive_network(&mut self, _msg: NetMessage) -> Handled {
            unimplemented!("No networking here!");
        }
    }

    #[test]
    fn test_immediate_blocking() {
        let system = KompactConfig::default().build().expect("System");
        let comp = system.create(BlockingComponent::new);
        system
            .start_notify(&comp)
            .wait_timeout(TIMEOUT)
            .expect("Component didn't start");
        comp.actor_ref().tell(BlockMe::Now);
        thread::sleep(TIMEOUT);
        system
            .kill_notify(comp.clone())
            .wait_timeout(TIMEOUT)
            .expect("Component didn't die");
        comp.on_definition(|cd| {
            assert_eq!(cd.test_string, "done");
        });
        system.shutdown().expect("shutdown");
    }

    #[test]
    fn test_channel_blocking() {
        let system = KompactConfig::default().build().expect("System");
        let comp = system.create(BlockingComponent::new);
        system
            .start_notify(&comp)
            .wait_timeout(TIMEOUT)
            .expect("Component didn't start");

        let (sender, receiver) = oneshot::channel();
        comp.actor_ref().tell(BlockMe::OnChannel(receiver));
        thread::sleep(TIMEOUT);
        sender.send("gotcha".to_string()).expect("Should have sent");
        thread::sleep(TIMEOUT);
        system
            .kill_notify(comp.clone())
            .wait_timeout(TIMEOUT)
            .expect("Component didn't die");
        comp.on_definition(|cd| {
            assert_eq!(cd.test_string, "gotcha");
        });
        system.shutdown().expect("shutdown");
    }

    #[test]
    fn test_mixed_blocking() {
        let system = KompactConfig::default().build().expect("System");
        let comp = system.create(BlockingComponent::new);
        system
            .start_notify(&comp)
            .wait_timeout(TIMEOUT)
            .expect("Component didn't start");

        let (sender, receiver) = oneshot::channel();
        comp.actor_ref().tell(BlockMe::OnChannel(receiver));
        thread::sleep(TIMEOUT);
        comp.actor_ref().tell(BlockMe::Now);
        sender.send("gotcha".to_string()).expect("Should have sent");
        thread::sleep(TIMEOUT);
        system
            .kill_notify(comp.clone())
            .wait_timeout(TIMEOUT)
            .expect("Component didn't die");
        comp.on_definition(|cd| {
            assert_eq!(cd.test_string, "done");
        });
        system.shutdown().expect("shutdown");
    }

    #[test]
    fn test_shutdown_blocking() {
        let system = KompactConfig::default().build().expect("System");
        let comp = system.create(BlockingComponent::new);
        system
            .start_notify(&comp)
            .wait_timeout(TIMEOUT)
            .expect("Component didn't start");
        comp.actor_ref().tell(BlockMe::OnShutdown);
        thread::sleep(TIMEOUT);
        system
            .kill_notify(comp.clone())
            .wait_timeout(TIMEOUT)
            .expect("Component didn't die");
        comp.on_definition(|cd| {
            assert_eq!(cd.test_string, "done");
        });
        system.shutdown().expect("shutdown");
    }

    #[test]
    fn test_component_spawn_off() -> () {
        let system = KompactConfig::default().build().expect("System");
        let comp = system.create(BlockingComponent::new);
        system
            .start_notify(&comp)
            .wait_timeout(TIMEOUT)
            .expect("Component didn't start");
        comp.actor_ref()
            .tell(BlockMe::SpawnOff("gotcha".to_string()));
        thread::sleep(TIMEOUT);
        system
            .kill_notify(comp.clone())
            .wait_timeout(TIMEOUT)
            .expect("Component didn't die");
        comp.on_definition(|cd| {
            assert_eq!(cd.test_string, "gotcha");
        });
        system.shutdown().expect("shutdown");
    }

    #[derive(Debug)]
    enum AsyncMe {
        Now,
        OnChannel(oneshot::Receiver<String>),
        ConcurrentMessage(oneshot::Receiver<String>),
        JustAMessage(String),
    }

    #[derive(ComponentDefinition)]
    struct AsyncComponent {
        ctx: ComponentContext<Self>,
        test_string: String,
    }

    impl AsyncComponent {
        fn new() -> Self {
            AsyncComponent {
                ctx: ComponentContext::uninitialised(),
                test_string: "started".to_string(),
            }
        }
    }

    ignore_lifecycle!(AsyncComponent);

    impl Actor for AsyncComponent {
        type Message = AsyncMe;

        fn receive_local(&mut self, msg: Self::Message) -> Handled {
            match msg {
                AsyncMe::Now => {
                    info!(self.log(), "Got AsyncMe::Now");
                    // async move closure syntax is nightly only
                    self.spawn_local(move |mut async_self| async move {
                        async_self.test_string = "done".to_string();
                        info!(async_self.log(), "Ran AsyncMe::Now future");
                        Handled::Ok
                    });
                    Handled::Ok
                }
                AsyncMe::OnChannel(receiver) => {
                    info!(self.log(), "Got AsyncMe::OnChannel");
                    // async move closure syntax is nightly only
                    self.spawn_local(move |mut async_self| async move {
                        info!(async_self.log(), "Started AsyncMe::OnChannel future");
                        let s = receiver.await;
                        async_self.test_string = s.expect("Some string");
                        info!(async_self.log(), "Completed Async::OnChannel future");
                        Handled::Ok
                    });
                    Handled::Ok
                }
                AsyncMe::ConcurrentMessage(receiver) => {
                    info!(self.log(), "Got AsyncMe::OnChannel");
                    // async move closure syntax is nightly only
                    self.spawn_local(move |mut async_self| async move {
                        info!(async_self.log(), "Started AsyncMe::ConcurrentMessag future");
                        let s = receiver.await.expect("Some string");
                        info!(
                            async_self.log(),
                            "Got message {} as state={}", s, async_self.test_string
                        );
                        assert_eq!(
                            s, async_self.test_string,
                            "Message was not processed before future!"
                        );
                        async_self.test_string = "done".to_string();
                        info!(
                            async_self.log(),
                            "Completed AsyncMe::ConcurrentMessage future with state={}",
                            async_self.test_string
                        );
                        Handled::Ok
                    });
                    Handled::Ok
                }
                AsyncMe::JustAMessage(s) => {
                    info!(self.log(), "Got AsyncMe::JustAMessage({})", s);
                    self.test_string = s;
                    Handled::Ok
                }
            }
        }

        fn receive_network(&mut self, _msg: NetMessage) -> Handled {
            unimplemented!("No networking here!");
        }
    }

    #[test]
    fn test_immediate_non_blocking() {
        let system = KompactConfig::default().build().expect("System");
        let comp = system.create(AsyncComponent::new);
        system
            .start_notify(&comp)
            .wait_timeout(TIMEOUT)
            .expect("Component didn't start");
        comp.actor_ref().tell(AsyncMe::Now);
        thread::sleep(TIMEOUT);
        system
            .kill_notify(comp.clone())
            .wait_timeout(TIMEOUT)
            .expect("Component didn't die");
        comp.on_definition(|cd| {
            assert_eq!(cd.test_string, "done");
        });
        system.shutdown().expect("shutdown");
    }

    #[test]
    fn test_channel_non_blocking() {
        let system = KompactConfig::default().build().expect("System");
        let comp = system.create(AsyncComponent::new);
        system
            .start_notify(&comp)
            .wait_timeout(TIMEOUT)
            .expect("Component didn't start");

        let (sender, receiver) = oneshot::channel();
        comp.actor_ref().tell(AsyncMe::OnChannel(receiver));
        thread::sleep(TIMEOUT);
        sender.send("gotcha".to_string()).expect("Should have sent");
        thread::sleep(TIMEOUT);
        system
            .kill_notify(comp.clone())
            .wait_timeout(TIMEOUT)
            .expect("Component didn't die");
        comp.on_definition(|cd| {
            assert_eq!(cd.test_string, "gotcha");
        });
        system.shutdown().expect("shutdown");
    }

    #[test]
    fn test_concurrent_non_blocking() {
        let system = KompactConfig::default().build().expect("System");
        let comp = system.create(AsyncComponent::new);
        system
            .start_notify(&comp)
            .wait_timeout(TIMEOUT)
            .expect("Component didn't start");

        let (sender, receiver) = oneshot::channel();
        comp.actor_ref().tell(AsyncMe::ConcurrentMessage(receiver));
        thread::sleep(TIMEOUT);
        let msg = "gotcha";
        comp.actor_ref()
            .tell(AsyncMe::JustAMessage(msg.to_string()));
        thread::sleep(TIMEOUT);
        sender.send(msg.to_string()).expect("Should have sent");
        thread::sleep(TIMEOUT);
        system
            .kill_notify(comp.clone())
            .wait_timeout(TIMEOUT)
            .expect("Component didn't die");
        comp.on_definition(|cd| {
            assert_eq!(cd.test_string, "done");
        });
        system.shutdown().expect("shutdown");
    }

    #[derive(Debug, Clone, Copy)]
    struct CountMe;
    #[derive(Debug, Clone, Copy)]
    struct Counted;
    #[derive(Debug, Clone, Copy)]
    struct SendCount;

    struct CounterPort;
    impl Port for CounterPort {
        type Indication = CountMe;
        type Request = Counted;
    }

    #[derive(ComponentDefinition)]
    struct CountSender {
        ctx: ComponentContext<Self>,
        count_port: ProvidedPort<CounterPort>,
        counted: usize,
    }
    impl Default for CountSender {
        fn default() -> Self {
            CountSender {
                ctx: ComponentContext::uninitialised(),
                count_port: ProvidedPort::uninitialised(),
                counted: 0,
            }
        }
    }
    ignore_lifecycle!(CountSender);
    impl Provide<CounterPort> for CountSender {
        fn handle(&mut self, _event: Counted) -> Handled {
            self.counted += 1;
            Handled::Ok
        }
    }
    impl Actor for CountSender {
        type Message = SendCount;

        fn receive_local(&mut self, _msg: Self::Message) -> Handled {
            self.count_port.trigger(CountMe);
            Handled::Ok
        }

        fn receive_network(&mut self, _msg: NetMessage) -> Handled {
            unimplemented!("No networking in this test");
        }
    }

    #[derive(ComponentDefinition)]
    struct Counter {
        ctx: ComponentContext<Self>,
        count_port: RequiredPort<CounterPort>,
        count: usize,
    }
    impl Default for Counter {
        fn default() -> Self {
            Counter {
                ctx: ComponentContext::uninitialised(),
                count_port: RequiredPort::uninitialised(),
                count: 0,
            }
        }
    }
    ignore_lifecycle!(Counter);
    impl Require<CounterPort> for Counter {
        fn handle(&mut self, _event: CountMe) -> Handled {
            self.count += 1;
            self.count_port.trigger(Counted);
            Handled::Ok
        }
    }
    impl Actor for Counter {
        type Message = Never;

        fn receive_local(&mut self, _msg: Self::Message) -> Handled {
            unreachable!("Never type is empty")
        }

        fn receive_network(&mut self, _msg: NetMessage) -> Handled {
            unimplemented!("No networking in this test");
        }
    }

    #[test]
    fn test_channel_disconnection() {
        let system = KompactConfig::default().build().expect("System");
        let sender = system.create(CountSender::default);

        let counter1 = system.create(Counter::default);
        let counter2 = system.create(Counter::default);

        let channel1: Box<(dyn Channel + Send + 'static)> =
            biconnect_components::<CounterPort, _, _>(&sender, &counter1)
                .expect("connection")
                .boxed();
        let channel2 =
            biconnect_components::<CounterPort, _, _>(&sender, &counter2).expect("connection");

        let start_all = || {
            let sender_start_f = system.start_notify(&sender);
            let counter1_start_f = system.start_notify(&counter1);
            let counter2_start_f = system.start_notify(&counter2);

            sender_start_f
                .wait_timeout(TIMEOUT)
                .expect("sender started");
            counter1_start_f
                .wait_timeout(TIMEOUT)
                .expect("counter1 started");
            counter2_start_f
                .wait_timeout(TIMEOUT)
                .expect("counter2 started");
        };
        start_all();

        sender.actor_ref().tell(SendCount);

        thread::sleep(TIMEOUT);

        let stop_all = || {
            let sender_stop_f = system.stop_notify(&sender);
            let counter1_stop_f = system.stop_notify(&counter1);
            let counter2_stop_f = system.stop_notify(&counter2);

            sender_stop_f.wait_timeout(TIMEOUT).expect("sender stopped");
            counter1_stop_f
                .wait_timeout(TIMEOUT)
                .expect("counter1 stopped");
            counter2_stop_f
                .wait_timeout(TIMEOUT)
                .expect("counter2 stopped");
        };
        stop_all();

        let check_counts = |sender_expected, counter1_expected, counter2_expected| {
            let sender_count = sender.on_definition(|cd| cd.counted);
            assert_eq!(sender_expected, sender_count);
            let counter1_count = counter1.on_definition(|cd| cd.count);
            assert_eq!(counter1_expected, counter1_count);
            let counter2_count = counter2.on_definition(|cd| cd.count);
            assert_eq!(counter2_expected, counter2_count);
        };
        check_counts(2, 1, 1);

        channel2.disconnect().expect("disconnect");

        start_all();

        sender.actor_ref().tell(SendCount);

        thread::sleep(TIMEOUT);

        stop_all();

        check_counts(3, 2, 1);

        channel1.disconnect().expect("disconnect");

        start_all();

        sender.actor_ref().tell(SendCount);

        thread::sleep(TIMEOUT);

        stop_all();

        check_counts(3, 2, 1);

        let sender_port: ProvidedRef<CounterPort> = sender.provided_ref();
        let counter1_port: RequiredRef<CounterPort> = counter1.required_ref();
        let channel1: Box<(dyn Channel + Send + 'static)> =
            sender.connect_to_required(counter1_port).boxed();
        let channel2 = counter2.connect_to_provided(sender_port);

        start_all();

        sender.actor_ref().tell(SendCount);

        thread::sleep(TIMEOUT);

        stop_all();

        check_counts(3, 3, 1);

        let counter2_port: RequiredRef<CounterPort> = counter2.required_ref();
        let channel3 = sender.connect_to_required(counter2_port);

        start_all();

        sender.actor_ref().tell(SendCount);

        thread::sleep(TIMEOUT);

        stop_all();

        check_counts(4, 4, 2);

        channel1.disconnect().expect("disconnected");
        channel2.disconnect().expect("disconnected");
        channel3.disconnect().expect("disconnected");

        start_all();

        sender.actor_ref().tell(SendCount);

        thread::sleep(TIMEOUT);

        stop_all();

        check_counts(4, 4, 2);

        system.shutdown().expect("shutdown");
    }
}

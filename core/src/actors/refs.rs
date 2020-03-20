use super::*;

use std::{fmt, ops::Deref};

/// The type of actor references for [dispatcher](Dispatcher) implementations
pub type DispatcherRef = ActorRefStrong<DispatchEnvelope>;

#[derive(Debug)]
pub(crate) struct TypedMsgQueue<M: MessageBounds> {
    inner: ConcurrentQueue<MsgEnvelope<M>>,
}
impl<M: MessageBounds> TypedMsgQueue<M> {
    pub(crate) fn new() -> TypedMsgQueue<M> {
        TypedMsgQueue {
            inner: ConcurrentQueue::new(),
        }
    }

    pub(crate) fn pop(&self) -> Option<MsgEnvelope<M>> {
        self.inner.pop().ok()
    }

    #[allow(unused)]
    pub(crate) fn into_dyn(q: Arc<Self>) -> Arc<dyn DynMsgQueue> {
        q as Arc<dyn DynMsgQueue>
    }

    pub(crate) fn into_dyn_weak(q: Weak<Self>) -> Weak<dyn DynMsgQueue> {
        q as Weak<dyn DynMsgQueue>
    }

    #[inline(always)]
    pub(crate) fn push(&self, value: MsgEnvelope<M>) {
        self.inner.push(value)
    }

    pub(crate) fn into_adapter<In: 'static>(
        q: Weak<Self>,
        convert: fn(In) -> M,
    ) -> Box<dyn AdapterQueue<In>> {
        let converting_queue = Box::new(ConvertingMsgQueue::from(q, convert));
        converting_queue as Box<dyn AdapterQueue<In>>
    }
}
impl<M: MessageBounds> DynMsgQueue for TypedMsgQueue<M> {
    // #[inline(always)]
    // fn push_dispatch(&self, value: DispatchEnvelope) {
    //     self.push(MsgEnvelope::Dispatch(value));
    // }

    #[inline(always)]
    fn push_net(&self, value: NetMessage) {
        self.push(MsgEnvelope::Net(value));
    }
}

pub(crate) trait DynMsgQueue: fmt::Debug + Sync + Send {
    //fn push_dispatch(&self, value: DispatchEnvelope);
    fn push_net(&self, value: NetMessage);
}
pub(crate) trait AdapterQueue<M>: fmt::Debug + Sync + Send {
    fn push_into(&self, value: M);
    fn box_clone(&self) -> Box<dyn AdapterQueue<M>>;
}

// /// Just a trait bound alias for a function that can be used in auto-wrapping queues.
// pub trait ConverterFunction<In, Out>: Fn(In) -> Out + Sync + Send {
//     type In;
//     type Out;
// }
// impl<In, Out, F> ConverterFunction<In, Out> for F
// where
//     F: Fn(In) -> Out + Sync + Send,
// {
//     // also empty
// }

pub(crate) struct ConvertingMsgQueue<In, Out>
where
    Out: MessageBounds,
{
    inner: Weak<TypedMsgQueue<Out>>,
    convert: fn(In) -> Out,
}
impl<In, Out> ConvertingMsgQueue<In, Out>
where
    Out: MessageBounds,
{
    pub(crate) fn from(
        inner: Weak<TypedMsgQueue<Out>>,
        convert: fn(In) -> Out,
    ) -> ConvertingMsgQueue<In, Out> {
        ConvertingMsgQueue { inner, convert }
    }

    fn convert(&self, value: In) -> Out {
        (self.convert)(value)
    }

    pub(crate) fn push(&self, value: In) {
        let out = self.convert(value);
        let msg = MsgEnvelope::Typed(out);
        match self.inner.upgrade() {
            Some(q) => q.push(msg),
            None =>
            {
                #[cfg(test)]
                println!("Dropping msg as target queueis unavailable: {:?}", msg)
            }
        }
    }
}
impl<In, Out> Clone for ConvertingMsgQueue<In, Out>
where
    Out: MessageBounds,
{
    fn clone(&self) -> Self {
        ConvertingMsgQueue {
            inner: self.inner.clone(),
            convert: self.convert,
        }
    }
}
impl<In, Out> AdapterQueue<In> for ConvertingMsgQueue<In, Out>
where
    In: 'static,
    Out: MessageBounds,
{
    fn push_into(&self, value: In) {
        self.push(value)
    }

    fn box_clone(&self) -> Box<dyn AdapterQueue<In>> {
        Box::new((*self).clone())
    }
}
impl<In, Out> fmt::Debug for ConvertingMsgQueue<In, Out>
where
    Out: MessageBounds,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "ConvertingMsgQueue{{...}}")
    }
}

/// A kind of actor reference that only allows [network messages](NetMessage) to be sent
///
/// For local-only dynamically typed actors, consider using `type Message = Box<Any>;`
/// together with `ActorRef<Box<Any>>` instead.
#[derive(Clone)]
pub struct DynActorRef {
    component: Weak<dyn CoreContainer>,
    msg_queue: Weak<dyn DynMsgQueue>,
}
impl DynActorRef {
    // pub(crate) fn from(
    //     component: Weak<dyn CoreContainer>,
    //     msg_queue: Weak<dyn DynMsgQueue>,
    // ) -> DynActorRef {
    //     DynActorRef {
    //         component,
    //         msg_queue,
    //     }
    // }

    pub(crate) fn enqueue(&self, msg: NetMessage) -> () {
        match (self.msg_queue.upgrade(), self.component.upgrade()) {
            (Some(q), Some(c)) => {
                let sd = c.core().increment_work();
                q.push_net(msg);
                match sd {
                    SchedulingDecision::Schedule => {
                        c.schedule();
                    }
                    _ => (), // nothing
                }
            }
            (_q, _c) =>
            {
                #[cfg(test)]
                println!(
                    "Dropping msg as target (queue? {:?}, component? {:?}) is unavailable: {:?}",
                    _q.is_some(),
                    _c.is_some(),
                    msg
                )
            }
        }
    }

    /// Attempts to upgrade the contained component, returning `true` if possible.
    pub(crate) fn can_upgrade_component(&self) -> bool {
        self.component.upgrade().is_some()
    }

    /// Send a network message to the target actor
    pub fn tell<I>(&self, v: I) -> ()
    where
        I: Into<NetMessage>,
    {
        let msg: NetMessage = v.into();
        self.enqueue(msg)
    }
}
impl fmt::Debug for DynActorRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "<dyn-actor-ref>")
    }
}
impl fmt::Display for DynActorRef {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(fmt, "<dyn-actor-ref>")
    }
}

impl PartialEq for DynActorRef {
    fn eq(&self, other: &DynActorRef) -> bool {
        match (self.component.upgrade(), other.component.upgrade()) {
            (Some(ref me), Some(ref it)) => Arc::ptr_eq(me, it),
            _ => false,
        }
    }
}

/// A version of [ActorRef](ActorRef) that prevents the target from being deallocated
///
/// Holding this kind of reference increases performance slightly,
/// compared to the normal [ActorRef](ActorRef). However, one must be careful
/// not to leave strong references lying around when not needed anymore, as they
/// prevent the target component from being deallocated, potentially causing memory creep.
///
/// Should be created via [hold](ActorRef::hold).
///
/// It is recommended to upgrade a normal actor reference to this variant,
/// if many messages will be sent in a loop to it. After the loop, the strong reference
/// can be dropped again.
pub struct ActorRefStrong<M: MessageBounds> {
    component: Arc<dyn CoreContainer>,
    msg_queue: Arc<TypedMsgQueue<M>>,
}

// Because the derive macro adds the wrong trait bound for this -.-
impl<M: MessageBounds> Clone for ActorRefStrong<M> {
    fn clone(&self) -> Self {
        ActorRefStrong {
            component: self.component.clone(),
            msg_queue: self.msg_queue.clone(),
        }
    }
}

impl<M: MessageBounds> ActorRefStrong<M> {
    pub(crate) fn enqueue(&self, env: MsgEnvelope<M>) -> () {
        let q = &self.msg_queue;
        let c = &self.component;
        let sd = c.core().increment_work();
        q.push(env);
        match sd {
            SchedulingDecision::Schedule => {
                c.schedule();
            }
            _ => (), // nothing
        }
    }

    /// Send message `v` to the actor instance referenced by this actor reference
    pub fn tell<I>(&self, v: I) -> ()
    where
        I: Into<M>,
    {
        let msg: M = v.into();
        let env = MsgEnvelope::Typed(msg);
        self.enqueue(env)
    }

    /// Helper to create messages that expect a response via a future instead of a message
    ///
    /// # Example
    ///
    /// ```
    /// use kompact::prelude::*;
    /// use std::time::Duration;
    ///
    /// #[derive(ComponentDefinition)]
    /// struct AskComponent {
    ///     ctx: ComponentContext<Self>
    /// }
    ///
    /// # impl AskComponent {
    /// #  fn new() -> AskComponent {
    /// #        AskComponent {
    /// #            ctx: ComponentContext::new()
    /// #        }
    /// #    }
    /// # }
    ///
    /// # ignore_control!(AskComponent);
    ///
    /// impl Actor for AskComponent {
    ///     type Message = Ask<u64, String>;
    ///
    ///     fn receive_local(&mut self, msg: Self::Message) -> () {
    ///         msg.complete(|num| {
    ///             format!("{}", num)
    ///         })
    ///         .expect("completion");
    ///     }
    ///
    /// #    fn receive_network(&mut self, _msg: NetMessage) -> () {
    /// #        unimplemented!("We don't care about this.");
    /// #    }
    /// }
    ///
    /// let system = KompactConfig::default().build().expect("system");
    /// let c = system.create(AskComponent::new);
    /// let strong_ref = c.actor_ref().hold().expect("live ref");
    ///
    /// let start_f = system.start_notify(&c);
    /// start_f
    ///     .wait_timeout(Duration::from_millis(1000))
    ///     .expect("component start");
    ///
    /// let response_f = strong_ref.ask(Ask::of(42u64));
    /// let response = response_f
    ///     .wait_timeout(Duration::from_millis(1000))
    ///     .expect("response");
    /// assert_eq!("42".to_string(), response);
    /// # system.shutdown().expect("shutdown");
    /// ```
    pub fn ask<R, F>(&self, f: F) -> Future<R>
    where
        R: Send + Sized,
        F: FnOnce(Promise<R>) -> M,
    {
        let (promise, future) = promise::<R>();
        let msg = f(promise);
        let env = MsgEnvelope::Typed(msg);
        self.enqueue(env);
        future
    }

    /// Downgrade this strong reference to a "normal" actor reference
    pub fn weak_ref(&self) -> ActorRef<M> {
        let c = Arc::downgrade(&self.component);
        let q = Arc::downgrade(&self.msg_queue);
        ActorRef {
            component: c,
            msg_queue: q,
        }
    }
}
impl<M: MessageBounds> ActorRefFactory for ActorRefStrong<M> {
    type Message = M;

    fn actor_ref(&self) -> ActorRef<M> {
        self.weak_ref()
    }
}

impl<M: MessageBounds> fmt::Debug for ActorRefStrong<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "<actor-ref-strong>")
    }
}

impl<M: MessageBounds> fmt::Display for ActorRefStrong<M> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(fmt, "<actor-ref-strong>")
    }
}

impl<M: MessageBounds> PartialEq for ActorRefStrong<M> {
    fn eq(&self, other: &ActorRefStrong<M>) -> bool {
        Arc::ptr_eq(&self.component, &other.component)
    }
}

/// A reference to an actor with `type Message = M;`
///
/// Use [tell](ActorRef::tell) to send messages to the referenced actor.
///
/// This kind of actor reference is similar to a [Weak](std::sync::Weak)
/// in that it doesn't prevent deallocation of the referenced actor.
/// If deallocation must be prevented for some reason, consider upgrading this to an
/// [ActorRefStrong](ActorRefStrong) using [hold](ActorRef::hold).
/// Note that a component can still enter the *destroyed* (or other non-active) state,
/// while a strong reference is held.
/// Holding a strong reference only prevent memory deallocation,
/// but does not guarantee messag handling.
///
/// Upgrading this to an [ActorRefStrong](ActorRefStrong) also improves performance somewhat,
/// and is recommended when using the same actor reference over and over again in a loop, for example.
pub struct ActorRef<M: MessageBounds> {
    component: Weak<dyn CoreContainer>,
    msg_queue: Weak<TypedMsgQueue<M>>,
}

// Because the derive macro adds the wrong trait bound for this -.-
impl<M: MessageBounds> Clone for ActorRef<M> {
    fn clone(&self) -> Self {
        ActorRef {
            component: self.component.clone(),
            msg_queue: self.msg_queue.clone(),
        }
    }
}

impl<M: MessageBounds> ActorRef<M> {
    pub(crate) fn new(
        component: Weak<dyn CoreContainer>,
        msg_queue: Weak<TypedMsgQueue<M>>,
    ) -> ActorRef<M> {
        ActorRef {
            component,
            msg_queue,
        }
    }

    pub(crate) fn enqueue(&self, env: MsgEnvelope<M>) -> () {
        match (self.msg_queue.upgrade(), self.component.upgrade()) {
            (Some(q), Some(c)) => {
                let sd = c.core().increment_work();
                q.push(env);
                match sd {
                    SchedulingDecision::Schedule => {
                        c.schedule();
                    }
                    _ => (), // nothing
                }
            }
            (_q, _c) =>
            {
                #[cfg(test)]
                println!(
                    "Dropping msg as target (queue? {:?}, component? {:?}) is unavailable: {:?}",
                    _q.is_some(),
                    _c.is_some(),
                    env
                )
            }
        }
    }

    /// Upgrade this reference to a strong reference
    ///
    /// This is only possible if the target actor has not already been
    /// deallocated. If it has, `None` will be returned.
    pub fn hold(&self) -> Option<ActorRefStrong<M>> {
        match (self.msg_queue.upgrade(), self.component.upgrade()) {
            (Some(q), Some(c)) => {
                let r = ActorRefStrong {
                    component: c,
                    msg_queue: q,
                };
                Some(r)
            }
            _ => None,
        }
    }

    /// Send message `v` to the actor instance referenced by this actor reference
    pub fn tell<I>(&self, v: I) -> ()
    where
        I: Into<M>,
    {
        let msg: M = v.into();
        let env = MsgEnvelope::Typed(msg);
        self.enqueue(env);
    }

    /// Helper to create messages that expect a response via a future instead of a message
    ///
    /// # Example
    ///
    /// ```
    /// use kompact::prelude::*;
    /// use std::time::Duration;
    ///
    /// #[derive(ComponentDefinition)]
    /// struct AskComponent {
    ///     ctx: ComponentContext<Self>
    /// }
    ///
    /// # impl AskComponent {
    /// #  fn new() -> AskComponent {
    /// #        AskComponent {
    /// #            ctx: ComponentContext::new()
    /// #        }
    /// #    }
    /// # }
    ///
    /// # ignore_control!(AskComponent);
    ///
    /// impl Actor for AskComponent {
    ///     type Message = Ask<u64, String>;
    ///
    ///     fn receive_local(&mut self, msg: Self::Message) -> () {
    ///         msg.complete(|num| {
    ///             format!("{}", num)
    ///         })
    ///         .expect("completion");
    ///     }
    ///
    /// #    fn receive_network(&mut self, _msg: NetMessage) -> () {
    /// #        unimplemented!("We don't care about this.");
    /// #    }
    /// }
    ///
    /// let system = KompactConfig::default().build().expect("system");
    /// let c = system.create(AskComponent::new);
    /// let actor_ref = c.actor_ref();
    ///
    /// let start_f = system.start_notify(&c);
    /// start_f
    ///     .wait_timeout(Duration::from_millis(1000))
    ///     .expect("component start");
    ///
    /// let response_f = actor_ref.ask(Ask::of(42u64));
    /// let response = response_f
    ///     .wait_timeout(Duration::from_millis(1000))
    ///     .expect("response");
    /// assert_eq!("42".to_string(), response);
    /// # system.shutdown().expect("shutdown");
    /// ```
    pub fn ask<R, F>(&self, f: F) -> Future<R>
    where
        R: Send + Sized,
        F: FnOnce(Promise<R>) -> M,
    {
        let (promise, future) = promise::<R>();
        let msg = f(promise);
        let env = MsgEnvelope::Typed(msg);
        self.enqueue(env);
        future
    }

    /// Returns a version of this actor ref that can only be used for [network messages](NetMessage),
    /// but not for `M`
    pub fn dyn_ref(&self) -> DynActorRef {
        DynActorRef {
            component: self.component.clone(),
            msg_queue: TypedMsgQueue::into_dyn_weak(self.msg_queue.clone()),
        }
    }

    /// Returns a version of this actor ref that can only be used to send `T`, which is then auto-wrapped into `M`
    ///
    /// Use this expose a narrower interface to another actor.
    ///
    /// # Note
    ///
    /// The indirection provided by `Recipient` has some performance impact.
    /// Use benchmarking to establish whether or not the better encapsulation is worth
    /// the performance loss in your scenario.
    pub fn recipient<T>(&self) -> Recipient<T>
    where
        T: Into<M> + fmt::Debug + 'static,
    {
        let adapter = TypedMsgQueue::into_adapter(self.msg_queue.clone(), Into::into);
        Recipient::from(self.component.clone(), adapter)
    }

    /// Returns a version of this actor ref that can only be used to send `T`, which is then auto-wrapped into `M`
    /// using `convert`
    ///
    /// Use this expose a narrower interface to another actor.
    ///
    /// # Note
    ///
    /// The indirection provided by `Recipient` has some performance impact.
    /// Use benchmarking to establish whether or not the better encapsulation is worth
    /// the performance loss in your scenario.
    pub fn recipient_with<T>(&self, convert: fn(T) -> M) -> Recipient<T>
    where
        T: fmt::Debug + 'static,
    {
        let adapter = TypedMsgQueue::into_adapter(self.msg_queue.clone(), convert);
        Recipient::from(self.component.clone(), adapter)
    }
}

impl<M: MessageBounds> ActorRefFactory for ActorRef<M> {
    type Message = M;

    fn actor_ref(&self) -> ActorRef<M> {
        self.clone()
    }
}

impl<M: MessageBounds> fmt::Debug for ActorRef<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "<actor-ref>")
    }
}

impl<M: MessageBounds> fmt::Display for ActorRef<M> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(fmt, "<actor-ref>")
    }
}

impl<M: MessageBounds> PartialEq for ActorRef<M> {
    fn eq(&self, other: &ActorRef<M>) -> bool {
        match (self.component.upgrade(), other.component.upgrade()) {
            (Some(ref me), Some(ref it)) => Arc::ptr_eq(me, it),
            _ => false,
        }
    }
}

/// A version of [ActorRef](ActorRef) that automatically converts `M` into some `M'`
/// as specified by the actor that created the `Recipient` from its own [ActorRef](ActorRef).
///
/// This can be used to expose only a subset of an actor's interface to another actor.
///
/// # Note
///
/// The indirection provided by this `Recipient` has some performance impact.
/// Use benchmarking to establish whether or not the better encapsulation is worth
/// the performance loss in your scenario.
pub struct Recipient<M> {
    component: Weak<dyn CoreContainer>,
    msg_queue: Box<dyn AdapterQueue<M>>,
}

impl<M: fmt::Debug> Recipient<M> {
    fn from(
        component: Weak<dyn CoreContainer>,
        msg_queue: Box<dyn AdapterQueue<M>>,
    ) -> Recipient<M> {
        Recipient {
            component,
            msg_queue,
        }
    }

    pub(crate) fn enqueue(&self, env: M) -> () {
        match self.component.upgrade() {
            Some(c) => {
                let sd = c.core().increment_work();
                self.msg_queue.push_into(env);
                match sd {
                    SchedulingDecision::Schedule => {
                        c.schedule();
                    }
                    _ => (), // nothing
                }
            }
            None =>
            {
                #[cfg(test)]
                println!("Dropping msg as target component is unavailable: {:?}", env)
            }
        }
    }

    /// Send message `v` to the actor instance referenced by this `Recipient`
    pub fn tell<I>(&self, v: I) -> ()
    where
        I: Into<M>,
    {
        let msg: M = v.into();
        self.enqueue(msg)
    }
}
// Because the derive macro adds the wrong trait bound for this -.-
impl<M> Clone for Recipient<M> {
    fn clone(&self) -> Self {
        Recipient {
            component: self.component.clone(),
            msg_queue: self.msg_queue.box_clone(),
        }
    }
}
impl<M> fmt::Debug for Recipient<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "<recipient>")
    }
}

impl<M> fmt::Display for Recipient<M> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(fmt, "<recipient>")
    }
}

impl<M> PartialEq for Recipient<M> {
    fn eq(&self, other: &Recipient<M>) -> bool {
        match (self.component.upgrade(), other.component.upgrade()) {
            (Some(ref me), Some(ref it)) => Arc::ptr_eq(me, it),
            _ => false,
        }
    }
}

/// A marker trait for messags that expect a response
///
/// Messages with this trait can be [replied](Request::reply) to.
pub trait Request: MessageBounds {
    /// The type of the response that is expected
    type Response;

    /// Fulfill this request by supplying a response of the appropriate type
    fn reply(&self, resp: Self::Response);
}

/// A generic type for request-response messages
///
/// This type contains information about the target actor
/// for the response, as well as the actual request itself.
///
/// Implementations can also be [dereferenced](std::ops::Deref::deref())
/// to the request message and [replied](Request::reply) to with a
/// response.
#[derive(Debug)]
pub struct WithSender<Req: MessageBounds, Resp: MessageBounds> {
    sender: ActorRef<Resp>,
    content: Req,
}
impl<Req: MessageBounds, Resp: MessageBounds> WithSender<Req, Resp> {
    /// Create a new instance from a request and an actor to reply to
    pub fn new(content: Req, sender: ActorRef<Resp>) -> WithSender<Req, Resp> {
        WithSender { sender, content }
    }

    /// Create a new instance from a request and something that can produce a reference to
    /// an actor to reply to
    ///
    /// This variant is convenient from within a component, as components and their contexts
    /// implement [ActorRefFactory](ActorRefFactory) for their actor message type.
    pub fn from(
        content: Req,
        sender: &dyn ActorRefFactory<Message = Resp>,
    ) -> WithSender<Req, Resp> {
        WithSender::new(content, sender.actor_ref())
    }

    /// Returns a reference to the response target actor
    pub fn sender(&self) -> &ActorRef<Resp> {
        &self.sender
    }

    /// Returns a reference to the content
    pub fn content(&self) -> &Req {
        &self.content
    }

    /// Takes only the content
    ///
    /// This prevents the request from being completed,
    /// as the `sender` is dropped!
    /// Use only after replying to the request.
    pub fn take_content(self) -> Req {
        self.content
    }
}
impl<Req: MessageBounds, Resp: MessageBounds> Request for WithSender<Req, Resp> {
    type Response = Resp;

    fn reply(&self, resp: Self::Response) {
        self.sender.tell(resp)
    }
}
impl<Req: MessageBounds, Resp: MessageBounds> Deref for WithSender<Req, Resp> {
    type Target = Req;

    fn deref(&self) -> &Self::Target {
        &self.content
    }
}

/// A generic type for request-response messages with a strong actor reference
///
/// This type contains information about the target actor
/// for the response, as well as the actual request itself.
/// It also prevents the response target actor from being deallocated.
/// Since that actor is waiting for a response, this is often a reasonable
/// choice and has performance benefits as well.
///
/// Implementations can also be [dereferenced](std::ops::Deref::deref())
/// to the request message and [replied](Request::reply) to with a
/// response.
#[derive(Debug)]
pub struct WithSenderStrong<Req: MessageBounds, Resp: MessageBounds> {
    sender: ActorRefStrong<Resp>,
    content: Req,
}
impl<Req: MessageBounds, Resp: MessageBounds> WithSenderStrong<Req, Resp> {
    /// Create a new instance from a request and an actor to reply to
    pub fn new(content: Req, sender: ActorRefStrong<Resp>) -> WithSenderStrong<Req, Resp> {
        WithSenderStrong { sender, content }
    }

    /// Create a new instance from a request and something that can produce a reference to
    /// an actor to reply to
    ///
    /// This variant is convenient from within a component, as components and their contexts
    /// implement [ActorRefFactory](ActorRefFactory) for their actor message type.
    pub fn from(
        content: Req,
        sender: &dyn ActorRefFactory<Message = Resp>,
    ) -> WithSenderStrong<Req, Resp> {
        WithSenderStrong::new(content, sender.actor_ref().hold().expect("Live ref"))
    }

    /// Returns a strong reference to the response target actor
    pub fn sender(&self) -> &ActorRefStrong<Resp> {
        &self.sender
    }

    /// Returns a reference to the content
    pub fn content(&self) -> &Req {
        &self.content
    }

    /// Takes only the content
    ///
    /// This prevents the request from being completed,
    /// as the `sender` is dropped!
    /// Use only after replying to the request.
    pub fn take_content(self) -> Req {
        self.content
    }
}
impl<Req: MessageBounds, Resp: MessageBounds> Request for WithSenderStrong<Req, Resp> {
    type Response = Resp;

    fn reply(&self, resp: Self::Response) {
        self.sender.tell(resp)
    }
}
impl<Req: MessageBounds, Resp: MessageBounds> Deref for WithSenderStrong<Req, Resp> {
    type Target = Req;

    fn deref(&self) -> &Self::Target {
        &self.content
    }
}

/// A generic type for request-response messages with a [Recipient](Recipient) as actor reference
///
/// This type contains narrowed information about the target actor
/// for the response, as well as the actual request itself.
///
/// Implementations can also be [dereferenced](std::ops::Deref::deref())
/// to the request message and [replied](Request::reply) to with a
/// response.
#[derive(Debug)]
pub struct WithRecipient<Req: MessageBounds, Resp: fmt::Debug + 'static> {
    sender: Recipient<Resp>,
    content: Req,
}
impl<Req: MessageBounds, Resp: fmt::Debug + 'static> WithRecipient<Req, Resp> {
    /// Create a new instance from a request and a recipient for the response
    pub fn new(content: Req, sender: Recipient<Resp>) -> WithRecipient<Req, Resp> {
        WithRecipient { sender, content }
    }

    /// Create a new instance from a request and something that can produce a reference to
    /// an actor to reply to
    ///
    /// This variant is convenient from within a component, as components and their contexts
    /// implement [ActorRefFactory](ActorRefFactory) for their actor message type.
    pub fn from<M>(
        content: Req,
        sender: &dyn ActorRefFactory<Message = M>,
    ) -> WithRecipient<Req, Resp>
    where
        M: MessageBounds + From<Resp>,
    {
        WithRecipient::new(content, sender.actor_ref().recipient())
    }

    /// Create a new instance from a request and something that can produce a reference to
    /// an actor to reply to, with custom `convert` function
    ///
    /// This variant is convenient from within a component, as components and their contexts
    /// implement [ActorRefFactory](ActorRefFactory) for their actor message type.
    pub fn from_convert<M: MessageBounds>(
        content: Req,
        sender: &dyn ActorRefFactory<Message = M>,
        convert: fn(Resp) -> M,
    ) -> WithRecipient<Req, Resp> {
        WithRecipient::new(content, sender.actor_ref().recipient_with(convert))
    }

    /// Returns a strong reference to the response target actor
    pub fn sender(&self) -> &Recipient<Resp> {
        &self.sender
    }

    /// Returns a reference to the content
    pub fn content(&self) -> &Req {
        &self.content
    }

    /// Takes only the content
    ///
    /// This prevents the request from being completed,
    /// as the `sender` is dropped!
    /// Use only after replying to the request.
    pub fn take_content(self) -> Req {
        self.content
    }
}
impl<Req: MessageBounds, Resp: fmt::Debug + 'static> Request for WithRecipient<Req, Resp> {
    type Response = Resp;

    fn reply(&self, resp: Self::Response) {
        self.sender.tell(resp)
    }
}
impl<Req: MessageBounds, Resp: fmt::Debug + 'static> Deref for WithRecipient<Req, Resp> {
    type Target = Req;

    fn deref(&self) -> &Self::Target {
        &self.content
    }
}

/// A factory trait for [recipients](Recipient)
///
/// This trait is blanket implemented for all [ActorRefFactory](ActorRefFactory)
/// implementations where the message type is `From<T>`.
pub trait Receiver<T> {
    /// Produce a recipient for `T`
    fn recipient(&self) -> Recipient<T>;
}
impl<M, T> Receiver<T> for dyn ActorRefFactory<Message = M>
where
    T: fmt::Debug + 'static,
    M: From<T> + MessageBounds,
{
    fn recipient(&self) -> Recipient<T> {
        self.actor_ref().recipient()
    }
}

#[cfg(test)]
mod tests {

    use crate::prelude::*;
    use std::{convert::From, sync::Arc, time::Duration};
    use synchronoise::CountdownEvent;

    #[test]
    fn test_recipient_explicit() {
        let system = KompactConfig::default().build().expect("KompactSystem");
        let latch = Arc::new(CountdownEvent::new(1));
        let latch2 = latch.clone();
        let ldactor = system.create(move || LatchDropActor::new(latch2));
        system.start(&ldactor);
        let ldref = ldactor.actor_ref();
        let ldrecipient: Recipient<Countdown> = ldref.recipient_with(|c| CountdownWrapper(c));
        ldrecipient.tell(Countdown);
        let count = latch.wait_timeout(Duration::from_millis(500));
        assert_eq!(count, 0, "Latch should have triggered by now!");
    }

    #[test]
    fn test_recipient_into() {
        let system = KompactConfig::default().build().expect("KompactSystem");
        let latch = Arc::new(CountdownEvent::new(1));
        let latch2 = latch.clone();
        let ldactor = system.create(move || LatchDropActor::new(latch2));
        system.start(&ldactor);
        let ldref = ldactor.actor_ref();
        let ldrecipient: Recipient<Countdown> = ldref.recipient();
        ldrecipient.tell(Countdown);
        let count = latch.wait_timeout(Duration::from_millis(500));
        assert_eq!(count, 0, "Latch should have triggered by now!");
    }

    #[derive(Debug)]
    struct Countdown;
    #[derive(Debug)]
    struct CountdownWrapper(Countdown);
    impl From<Countdown> for CountdownWrapper {
        fn from(c: Countdown) -> Self {
            CountdownWrapper(c)
        }
    }

    #[derive(ComponentDefinition)]
    struct LatchDropActor {
        ctx: ComponentContext<Self>,
        latch: Arc<CountdownEvent>,
    }
    impl LatchDropActor {
        fn new(latch: Arc<CountdownEvent>) -> LatchDropActor {
            LatchDropActor {
                ctx: ComponentContext::new(),
                latch,
            }
        }
    }
    impl Provide<ControlPort> for LatchDropActor {
        fn handle(&mut self, _event: ControlEvent) -> () {
            // ignore
        }
    }
    impl Actor for LatchDropActor {
        type Message = CountdownWrapper;

        fn receive_local(&mut self, _msg: Self::Message) -> () {
            self.latch
                .decrement()
                .expect("Latch should have decremented!");
        }

        /// Handles (serialised or reflected) messages from the network.
        fn receive_network(&mut self, _msg: NetMessage) -> () {
            unimplemented!();
        }
    }
}

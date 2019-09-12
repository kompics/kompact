use super::*;

use std::fmt;

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
    pub(crate) fn as_dyn(q: Arc<Self>) -> Arc<dyn DynMsgQueue> {
        q as Arc<dyn DynMsgQueue>
    }

    pub(crate) fn as_dyn_weak(q: Weak<Self>) -> Weak<dyn DynMsgQueue> {
        q as Weak<dyn DynMsgQueue>
    }

    #[inline(always)]
    pub(crate) fn push(&self, value: MsgEnvelope<M>) {
        self.inner.push(value)
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

#[derive(Clone)]
pub struct DynActorRef {
    component: Weak<dyn CoreContainer>,
    msg_queue: Weak<dyn DynMsgQueue>,
}
impl DynActorRef {
    pub(crate) fn from(
        component: Weak<dyn CoreContainer>,
        msg_queue: Weak<dyn DynMsgQueue>,
    ) -> DynActorRef {
        DynActorRef {
            component,
            msg_queue,
        }
    }

    pub(crate) fn enqueue(&self, msg: NetMessage) -> () {
        match (self.msg_queue.upgrade(), self.component.upgrade()) {
            (Some(q), Some(c)) => {
                let sd = c.core().increment_work();
                q.push_net(msg);
                match sd {
                    SchedulingDecision::Schedule => {
                        let system = c.core().system();
                        system.schedule(c.clone());
                    }
                    _ => (), // nothing
                }
            }
            (q, c) => println!(
                "Dropping msg as target (queue? {:?}, component? {:?}) is unavailable: {:?}",
                q.is_some(),
                c.is_some(),
                msg
            ),
        }
    }

    /// Attempts to upgrade the contained component, returning `true` if possible.
    pub(crate) fn can_upgrade_component(&self) -> bool {
        self.component.upgrade().is_some()
    }

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
                let system = c.core().system();
                system.schedule(c.clone());
            }
            _ => (), // nothing
        }
    }

    pub fn tell<I>(&self, v: I) -> ()
    where
        I: Into<M>,
    {
        let msg: M = v.into();
        let env = MsgEnvelope::Typed(msg);
        self.enqueue(env)
    }

    pub fn weak_ref(&self) -> ActorRef<M> {
        let c = Arc::downgrade(&self.component);
        let q = Arc::downgrade(&self.msg_queue);
        ActorRef {
            component: c,
            msg_queue: q,
        }
    }
}
impl<M: MessageBounds> ActorRefFactory<M> for ActorRefStrong<M> {
    fn actor_ref(&self) -> ActorRef<M> {
        self.weak_ref()
    }
}
impl<M: MessageBounds> DynActorRefFactory for ActorRefStrong<M> {
    fn dyn_ref(&self) -> DynActorRef {
        self.weak_ref().dyn_ref()
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
                        let system = c.core().system();
                        system.schedule(c.clone());
                    }
                    _ => (), // nothing
                }
            }
            (q, c) => println!(
                "Dropping msg as target (queue? {:?}, component? {:?}) is unavailable: {:?}",
                q.is_some(),
                c.is_some(),
                env
            ),
        }
    }

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

    pub fn tell<I>(&self, v: I) -> ()
    where
        I: Into<M>,
    {
        let msg: M = v.into();
        let env = MsgEnvelope::Typed(msg);
        self.enqueue(env)
    }

    // not used anymore
    // /// Attempts to upgrade the contained component, returning `true` if possible.
    // pub(crate) fn can_upgrade_component(&self) -> bool {
    //     self.component.upgrade().is_some()
    // }

    /// Returns a version of this actor ref that can only be used for `NetworkMessage`, but not `M`.
    pub fn dyn_ref(&self) -> DynActorRef {
        DynActorRef {
            component: self.component.clone(),
            msg_queue: TypedMsgQueue::as_dyn_weak(self.msg_queue.clone()),
        }
    }
}

impl<M: MessageBounds> ActorRefFactory<M> for ActorRef<M> {
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

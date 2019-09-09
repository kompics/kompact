use super::*;
use crate::messaging::{CastEnvelope, DispatchEnvelope, Message, MsgEnvelope, ReceiveEnvelope};
use bytes::{Buf, IntoBuf};
use std::{
    any::Any,
    fmt::{self, Debug},
    sync::{Arc, Weak},
};

mod paths;
pub use paths::*;

/// Handles raw message envelopes.
/// Usually it's better to us the unwrapped functions in `Actor`, but this can be more efficient at times.
pub trait ActorRaw: ExecuteSend {
    fn receive(&mut self, env: ReceiveEnvelope) -> ();
}

/// Handles both local and networked messages.
pub trait Actor {
    /// Handles local messages.
    /// Note that the default implementation for `ActorRaw` based on this `Actor` will deallocate
    /// owned (`Box<Any>`) and shared (`Arc<Any>`) references after this method finishes.
    /// If you want to keep the message and really don't want to pay the cost of a clone invokation here,
    /// you must implement `ActorRaw` instead.
    fn receive_local(&mut self, sender: ActorRef, msg: &dyn Any) -> ();

    /// Handles (serialised) messages from the network.
    fn receive_message(&mut self, sender: ActorPath, ser_id: u64, buf: &mut dyn Buf) -> ();
}

/// A dispatcher is a system component that knows how to route messages and create system paths.
pub trait Dispatcher: ExecuteSend {
    fn receive(&mut self, env: DispatchEnvelope) -> ();
    fn system_path(&mut self) -> SystemPath;
}

impl<CD> ActorRaw for CD
where
    CD: Actor,
{
    fn receive(&mut self, env: ReceiveEnvelope) -> () {
        match env {
            ReceiveEnvelope::Cast(c) => match c.msg {
                Message::StaticRef(v) => self.receive_local(c.src, v),
                Message::Owned(v) => self.receive_local(c.src, v.as_ref()),
                Message::Shared(v) => self.receive_local(c.src, v.as_ref()),
            },
            ReceiveEnvelope::Msg {
                src,
                dst: _,
                ser_id,
                data,
            } => self.receive_message(src, ser_id, &mut data.into_buf()),
        }
    }
}

pub trait ActorRefFactory {
    fn actor_ref(&self) -> ActorRef;
}

pub trait Dispatching {
    fn dispatcher_ref(&self) -> ActorRef;
}

#[derive(Clone)]
pub struct ActorRefStrong {
    component: Arc<dyn CoreContainer>,
    msg_queue: Arc<ConcurrentQueue<MsgEnvelope>>,
}

impl ActorRefStrong {
    pub(crate) fn enqueue(&self, env: MsgEnvelope) -> () {
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

    pub fn tell<I, S>(&self, v: I, from: &S) -> ()
    where
        I: Into<Message>,
        S: ActorRefFactory,
    {
        let msg: Message = v.into();
        let env = DispatchEnvelope::Cast(CastEnvelope {
            src: from.actor_ref(),
            msg,
        });
        self.enqueue(MsgEnvelope::Dispatch(env))
    }
}

#[derive(Clone)]
pub struct ActorRef {
    component: Weak<dyn CoreContainer>,
    msg_queue: Weak<ConcurrentQueue<MsgEnvelope>>,
}

impl ActorRef {
    pub(crate) fn new(
        component: Weak<dyn CoreContainer>,
        msg_queue: Weak<ConcurrentQueue<MsgEnvelope>>,
    ) -> ActorRef {
        ActorRef {
            component,
            msg_queue,
        }
    }

    pub(crate) fn enqueue(&self, env: MsgEnvelope) -> () {
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

    pub fn hold(&self) -> Option<ActorRefStrong> {
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

    pub fn tell<I, S>(&self, v: I, from: &S) -> ()
    where
        I: Into<Message>,
        S: ActorRefFactory,
    {
        let msg: Message = v.into();
        let env = DispatchEnvelope::Cast(CastEnvelope {
            src: from.actor_ref(),
            msg,
        });
        self.enqueue(MsgEnvelope::Dispatch(env))
    }

    // // TODO figure out a way to have one function match both cases -.-
    // pub fn tell_any<S>(&self, v: Box<Any + Send>, from: &S) -> ()
    // where
    //     S: ActorRefFactory,
    // {
    //     let env = DispatchEnvelope::Cast(CastEnvelope {
    //         src: from.actor_ref(),
    //         msg,
    //     });
    //     self.enqueue(MsgEnvelope::Dispatch(env))
    // }

    /// Attempts to upgrade the contained component, returning `true` if possible.
    pub(crate) fn can_upgrade_component(&self) -> bool {
        self.component.upgrade().is_some()
    }
}

impl ActorRefFactory for ActorRef {
    fn actor_ref(&self) -> ActorRef {
        self.clone()
    }
}

impl Debug for ActorRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "<actor-ref>")
    }
}

impl fmt::Display for ActorRef {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(fmt, "<actor-ref>")
    }
}

impl PartialEq for ActorRef {
    fn eq(&self, other: &ActorRef) -> bool {
        match (self.component.upgrade(), other.component.upgrade()) {
            (Some(ref me), Some(ref it)) => Arc::ptr_eq(me, it),
            _ => false,
        }
    }
}

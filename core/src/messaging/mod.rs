//! Messaging types for sending and receiving messages between remote actors.

use crate::actors::ActorPath;
use crate::actors::ActorRef;
use crate::net::events::NetworkEvent;
use crate::serialisation::Serialisable;
use crate::utils;
use bytes::Bytes;
use std::any::Any;
use std::fmt;
use std::sync::Arc;
use uuid::Uuid;

pub mod framing;

/// Abstracts over the exact memory allocation of dynamic messages being sent.
pub enum Message {
    StaticRef(&'static (dyn Any + Sync)),
    Owned(Box<dyn Any + Send>),
    Shared(Arc<dyn Any + Sync + Send>),
}
impl fmt::Debug for Message {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Message::StaticRef(_) => write!(f, "Message(&'static Any)"),
            Message::Owned(_) => write!(f, "Message(Box<Any>)"),
            Message::Shared(_) => write!(f, "Message(Arc<Any>)"),
        }
    }
}

impl<T: Sync> From<&'static T> for Message {
    fn from(v: &'static T) -> Self {
        Message::StaticRef(v)
    }
}

impl<T: Send + 'static> From<Box<T>> for Message {
    fn from(v: Box<T>) -> Self {
        Message::Owned(v as Box<dyn Any + Send>)
    }
}

impl From<Box<dyn Any + Send>> for Message {
    fn from(v: Box<dyn Any + Send>) -> Self {
        Message::Owned(v)
    }
}

impl<T: Send + Sync + 'static> From<Arc<T>> for Message {
    fn from(v: Arc<T>) -> Self {
        Message::Shared(v as Arc<dyn Any + Sync + Send>)
    }
}

impl From<Arc<dyn Any + Sync + Send>> for Message {
    fn from(v: Arc<dyn Any + Sync + Send>) -> Self {
        Message::Shared(v)
    }
}

#[derive(Debug)]
pub enum MsgEnvelope {
    Dispatch(DispatchEnvelope),
    Receive(ReceiveEnvelope),
}

#[derive(Debug)]
pub struct CastEnvelope {
    pub src: ActorRef,
    pub msg: Message,
}

#[derive(Debug, PartialEq)]
pub enum RegistrationError {
    DuplicateEntry,
    Unsupported,
}

/// Envelope representing an actor registration event.
///
/// Used for registering an [ActorRef](actors::ActorRef) with an [ActorPath](actors::ActorPath).
#[derive(Debug)]
pub enum RegistrationEnvelope {
    Register(
        ActorRef,
        PathResolvable,
        Option<utils::Promise<Result<ActorPath, RegistrationError>>>,
    ),
}

/// Envelopes destined for the dispatcher
#[derive(Debug)]
pub enum DispatchEnvelope {
    Cast(CastEnvelope),
    Msg {
        src: PathResolvable,
        dst: ActorPath,
        msg: Box<dyn Serialisable>,
    },
    Registration(RegistrationEnvelope),
    Event(EventEnvelope),
}

#[derive(Debug)]
pub enum EventEnvelope {
    Network(NetworkEvent),
}

#[derive(Debug)]
pub enum ReceiveEnvelope {
    Cast(CastEnvelope),
    Msg {
        src: ActorPath,
        dst: ActorPath,
        ser_id: u64,
        data: Bytes,
    },
}

#[derive(Debug, Clone)]
pub enum PathResolvable {
    Path(ActorPath),
    ActorId(Uuid),
    Alias(String),
    System,
}

impl ReceiveEnvelope {
    pub fn dst(&self) -> &ActorPath {
        match self {
            ReceiveEnvelope::Cast(_) => unimplemented!(),
            ReceiveEnvelope::Msg { src: _, dst, .. } => &dst,
        }
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    // fn force_sync<S: Sync>(_s: S) -> () {
    //     // do nothing
    // }

    fn force_send<S: Send>(_s: S) -> () {
        // do nothing
    }

    const TEST_V: u64 = 123456;

    #[test]
    fn force_send_inference() {
        let msg1 = Message::StaticRef(&TEST_V);
        force_send(msg1);
        let msg2 = Message::Owned(Box::new(TEST_V));
        force_send(msg2);
        let msg3 = Message::Shared(Arc::new(TEST_V));
        force_send(msg3);
    }
}

//! Messaging types for sending and receiving messages between remote actors.

use crate::actors::ActorPath;
use crate::actors::ActorRef;
use bytes::Bytes;
use crate::net::events::NetworkEvent;
use crate::serialisation::Serialisable;
use std::any::Any;
use crate::utils;
use uuid::Uuid;

pub mod framing;

#[derive(Debug)]
pub enum MsgEnvelope {
    Dispatch(DispatchEnvelope),
    Receive(ReceiveEnvelope),
}

#[derive(Debug)]
pub struct CastEnvelope {
    pub(crate) src: ActorRef,
    pub(crate) v: Box<Any + Send>,
}

#[derive(Debug, PartialEq)]
pub enum RegistrationError {
    DuplicateEntry,
    Unsupported,
}

/// Envelope representing an actor registration event.
///
/// Used for registering an [ActorRef](actors::ActorRef) with a path.
#[derive(Debug)]
pub enum RegistrationEnvelope {
    Register(
        ActorRef,
        PathResolvable,
        Option<utils::Promise<Result<(), RegistrationError>>>,
    ),
}

/// Envelopes destined for the dispatcher
#[derive(Debug)]
pub enum DispatchEnvelope {
    Cast(CastEnvelope),
    Msg {
        src: PathResolvable,
        dst: ActorPath,
        msg: Box<Serialisable>,
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

#[derive(Debug)]
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

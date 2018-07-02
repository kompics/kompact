//! Messaging types for sending and receiving messages between remote actors.

use actors::ActorPath;
use actors::ActorRef;
use bytes::Bytes;
use serialisation::Serialisable;
use std::any::Any;

#[derive(Debug)]
pub enum MsgEnvelope {
    Dispatch(DispatchEnvelope),
    Receive(ReceiveEnvelope),
}

#[derive(Debug)]
pub struct CastEnvelope {
    pub(crate) src: ActorRef,
    pub(crate) v: Box<Any>,
}

/// Envelope representing an actor registration event.
///
/// Used for registering and deregistering an [ActorPath](actors::ActorPath) with a name.
#[derive(Debug)]
pub enum RegistrationEnvelope {
    Register(ActorRef),
    Deregister(ActorRef),
}

#[derive(Debug)]
pub enum DispatchEnvelope {
    Cast(CastEnvelope),
    Msg {
        src: ActorPath,
        dst: ActorPath,
        msg: Box<Serialisable>,
    },
    Registration(RegistrationEnvelope),
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

//! Messaging types for sending and receiving messages between remote actors.

use crate::{
    actors::{ActorPath, DynActorRef, MessageBounds},
    net::events::NetworkEvent,
    serialisation::{Deserialiser, SerError, SerId, Serialisable},
    utils,
};
use bytes::{Bytes, IntoBuf};
use std::any::Any;
use uuid::Uuid;

pub mod framing;

#[derive(Debug)]
pub struct NetMessage {
    ser_id: SerId,
    sender: ActorPath,
    receiver: ActorPath,
    data: HeapOrSer,
}
#[derive(Debug)]
pub enum HeapOrSer {
    Boxed(Box<dyn Any + Send + 'static>),
    Serialised(bytes::Bytes),
}
impl NetMessage {
    pub fn with_box(
        ser_id: SerId,
        sender: ActorPath,
        receiver: ActorPath,
        data: Box<dyn Any + Send + 'static>,
    ) -> NetMessage {
        NetMessage {
            ser_id,
            sender,
            receiver,
            data: HeapOrSer::Boxed(data),
        }
    }

    pub fn with_bytes(
        ser_id: SerId,
        sender: ActorPath,
        receiver: ActorPath,
        data: Bytes,
    ) -> NetMessage {
        NetMessage {
            ser_id,
            sender,
            receiver,
            data: HeapOrSer::Serialised(data),
        }
    }

    pub fn try_deserialise<T: 'static, D>(self) -> Result<T, UnpackError<Self>>
    where
        D: Deserialiser<T>,
    {
        if self.ser_id == D::SER_ID {
            let NetMessage {
                ser_id,
                sender,
                receiver,
                data,
            } = self;
            match data {
                HeapOrSer::Boxed(b) => b
                    .downcast::<T>()
                    .map(|b| *b)
                    .map_err(|b| UnpackError::NoCast(Self::with_box(ser_id, sender, receiver, b))),
                HeapOrSer::Serialised(buf) => {
                    D::deserialise(&mut buf.into_buf()).map_err(|e| UnpackError::DeserError(e))
                }
            }
        } else {
            Err(UnpackError::NoIdMatch(self))
        }
    }

    pub fn ser_id(&self) -> &SerId {
        &self.ser_id
    }

    pub fn sender(&self) -> &ActorPath {
        &self.sender
    }

    pub fn receiver(&self) -> &ActorPath {
        &self.receiver
    }

    pub fn data(&self) -> &HeapOrSer {
        &self.data
    }
}

/// An error that is thrown when deserialisation of a [NetMessage](NetMessage) is attempted.
#[derive(Debug)]
pub enum UnpackError<T> {
    /// `ser_id` did not match the given [Deserialiser](crate::serialisation::Deserialiser).
    NoIdMatch(T),
    /// `Box<dyn Any>::downcast()` failed.
    NoCast(T),
    /// An error occurred during deserialisation of the buffer.
    ///
    /// This can not contain `T`, as the buffer may have been corrupted during the failed attempt.
    DeserError(SerError),
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
        DynActorRef,
        PathResolvable,
        Option<utils::Promise<Result<ActorPath, RegistrationError>>>,
    ),
}

/// Envelopes destined for the dispatcher
#[derive(Debug)]
pub enum DispatchEnvelope {
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
pub enum MsgEnvelope<M: MessageBounds> {
    Typed(M),
    Net(NetMessage),
}

#[derive(Debug, Clone)]
pub enum PathResolvable {
    Path(ActorPath),
    ActorId(Uuid),
    Alias(String),
    System,
}

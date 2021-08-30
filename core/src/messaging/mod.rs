//! Messaging types for sending and receiving messages between remote actors.

use crate::{
    actors::{ActorPath, DynActorRef, MessageBounds, PathParseError},
    net::{
        buffers::{BufferChunk, BufferEncoder, ChunkLease, ChunkRef},
        frames::FRAME_HEAD_LEN,
    },
    serialisation::{
        ser_helpers::{deserialise_chunk_lease, deserialise_chunk_ref},
        Deserialiser,
        SerError,
        SerId,
        Serialisable,
        Serialiser,
        TryClone,
    },
    utils,
};
use bytes::{Buf, Bytes};
use std::{any::Any, convert::TryFrom, ops::Deref, str::FromStr};
use uuid::Uuid;
mod net_message;
pub use net_message::*;
mod registration;
pub use registration::*;
mod serialised;
pub use serialised::*;
pub(crate) mod dispatch;
pub use dispatch::*;
mod deser_macro;
use crate::{net::SocketAddr, prelude::NetworkStatus};
pub use deser_macro::*;

pub mod framing;

/// An event from the network
///
/// This is left as an enum for future extension.
#[derive(Debug)]
pub enum EventEnvelope {
    /// An event from the network
    Network(NetworkStatus),
    /// Rejected DispatchData, NetworkThread is unable to send it
    RejectedData((SocketAddr, Box<DispatchData>)),
}

/// A message that is accepted by an actor's mailbox
#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum MsgEnvelope<M: MessageBounds> {
    /// A message of the actor's `Message` type
    Typed(M),
    /// A message from the network
    Net(NetMessage),
}

/// Something that can resolved to some kind of path by the dispatcher
#[derive(Debug, Clone)]
pub enum PathResolvable {
    /// An actual actor path
    Path(ActorPath),
    /// The unique id of an actor
    ///
    /// Can be resolved to a [unique path](ActorPath::Unique) in the current system.
    ActorId(Uuid),
    /// An actor path alias with all segments merged
    ///
    /// Can be resolved to a [named path](ActorPath::Named) in the current system.
    Alias(String),
    /// An actor path alias with all segments individually
    ///
    /// Can be resolved to a [named path](ActorPath::Named) in the current system.
    Segments(Vec<String>),
    /// The system path (as provided by the dispatcher)
    System,
}

impl From<ActorPath> for PathResolvable {
    fn from(path: ActorPath) -> Self {
        PathResolvable::Path(path)
    }
}
impl TryFrom<String> for PathResolvable {
    type Error = PathParseError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let parsed = crate::actors::parse_path(&value);
        crate::actors::validate_lookup_path(&parsed).map(|_| PathResolvable::Alias(value))
    }
}
impl FromStr for PathResolvable {
    type Err = PathParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let parsed = crate::actors::parse_path(value);
        crate::actors::validate_lookup_path(&parsed)
            .map(|_| PathResolvable::Alias(value.to_string()))
    }
}

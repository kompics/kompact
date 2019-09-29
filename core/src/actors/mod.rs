use super::*;
use crate::messaging::{DispatchEnvelope, MsgEnvelope, NetMessage, UnpackError};
use std::{
    fmt,
    sync::{Arc, Weak},
};

mod paths;
mod refs;
pub use paths::*;
pub use refs::*;

/// Just a trait alias hack.
pub trait MessageBounds: fmt::Debug + Send + 'static
where
    Self: std::marker::Sized,
{
    // Trait aliases need no methods
}
impl<M> MessageBounds for M
where
    M: fmt::Debug + Send + 'static,
{
    // Nothing to implement
}

/// Handles raw message envelopes.
/// Usually it's better to us the unwrapped functions in `Actor`, but this can be more efficient at times.
pub trait ActorRaw {
    type Message: MessageBounds;

    fn receive(&mut self, env: MsgEnvelope<Self::Message>) -> ();
}

/// Handles both local and networked messages.
pub trait Actor {
    type Message: MessageBounds;

    /// Handles local messages.
    fn receive_local(&mut self, msg: Self::Message) -> ();

    /// Handles (serialised or reflected) messages from the network.
    fn receive_network(&mut self, msg: NetMessage) -> ();
}

/// A dispatcher is a system component that knows how to route messages and create system paths.
pub trait Dispatcher: ActorRaw<Message = DispatchEnvelope> {
    fn system_path(&mut self) -> SystemPath;
}

impl<A, M: MessageBounds> ActorRaw for A
where
    A: Actor<Message = M>,
{
    type Message = M;

    #[inline(always)]
    fn receive(&mut self, env: MsgEnvelope<M>) -> () {
        match env {
            MsgEnvelope::Typed(m) => self.receive_local(m),
            MsgEnvelope::Net(nm) => self.receive_network(nm),
        }
    }
}

pub trait ActorRefFactory<M: MessageBounds> {
    fn actor_ref(&self) -> ActorRef<M>;
}

pub trait DynActorRefFactory {
    fn dyn_ref(&self) -> DynActorRef;
}

// Impossible in Rust...must provide individual implementations for every F -.-
// impl<M: MessageBounds, F: ActorRefFactory<M>> DynActorRefFactory for F {
//     fn dyn_ref(&self) -> DynActorRef {
//         self.actor_ref().dyn_ref()
//     }
// }

pub trait Dispatching {
    fn dispatcher_ref(&self) -> DispatcherRef;
}

pub trait NetworkActor: ComponentLogging {
    type Message: MessageBounds;
    type Deserialiser: Deserialiser<Self::Message>;

    /// Handles all messages, after deserialisation.
    ///
    /// `sender` will only be supplied if the original message was a [NetMessage](crate::messaging::NetMessage).
    fn receive(&mut self, sender: Option<ActorPath>, msg: Self::Message) -> ();

    fn on_error(&mut self, error: UnpackError<NetMessage>) -> () {
        warn!(
            self.log(),
            "Could not deserialise a message with Deserialiser with id={}. Error was: {:?}",
            Self::Deserialiser::SER_ID,
            error
        );
    }
}

impl<A, M, D> Actor for A
where
    M: MessageBounds,
    D: Deserialiser<M>,
    A: NetworkActor<Message = M, Deserialiser = D>,
{
    type Message = M;

    #[inline(always)]
    fn receive_local(&mut self, msg: Self::Message) -> () {
        self.receive(None, msg)
    }

    #[inline(always)]
    fn receive_network(&mut self, msg: NetMessage) -> () {
        let sender = msg.sender().clone();
        match msg.try_deserialise::<_, <Self as NetworkActor>::Deserialiser>() {
            Ok(m) => self.receive(Some(sender), m),
            Err(e) => self.on_error(e),
        }
    }
}

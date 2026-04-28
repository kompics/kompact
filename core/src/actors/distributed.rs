use super::*;
use crate::{component::HandlerResult, messaging::UnpackError};

/// A trait for things that can deal with [network messages](NetMessage)
pub trait DynActorRefFactory {
    /// Returns a version of an actor ref that can only be used for [network messages](crate::messaging::NetMessage),
    /// but not for typed messages.
    fn dyn_ref(&self) -> DynActorRef;
}

/// A trait for accessing [dispatcher references](DispatcherRef)
pub trait Dispatching {
    /// Returns the associated dispatcher reference.
    fn dispatcher_ref(&self) -> DispatcherRef;
}

/// A trait for actors that handle the same set of messages locally and remotely
///
/// Implementing this trait is roughly equivalent to the default assumptions in most
/// actor system implementations with a distributed runtime (Erlang, Akka, Orelans, etc.).
///
/// # Example
///
/// This implementation uses the trivial default [Deserialiser](Deserialiser)
/// for the unit type `()` from the `serialisation::default_serialisiers` module
/// (which is imported by default with the prelude).
///
/// ```
/// use kompact::prelude::*;
///
/// #[derive(ComponentDefinition)]
/// struct NetActor {
///    ctx: ComponentContext<Self>
/// }
/// ignore_lifecycle!(NetActor);
/// impl NetworkActor for NetActor {
///     type Message = ();
///     type Deserialiser = ();
///
///     fn receive(&mut self, sender: Option<ActorPath>, msg: Self::Message) -> HandlerResult {
///         info!(self.log(), "Got a local or deserialised remote message: {:?}", msg);
///         Handled::OK
///     }
/// }
/// ```
pub trait NetworkActor: ComponentLogging {
    /// The type of messages the actor accepts.
    type Message: MessageBounds;
    /// The deserialiser used to unpack [network messages](NetMessage)
    /// into `Self::Message`.
    type Deserialiser: Deserialiser<Self::Message>;

    /// Handles all messages after deserialisation.
    ///
    /// The `sender` argument will only be supplied if the original message
    /// was a [NetMessage](crate::messaging::NetMessage), otherwise it's `None`.
    fn receive(&mut self, sender: Option<ActorPath>, msg: Self::Message) -> HandlerResult;

    /// Handle errors during unpacking of network messages.
    ///
    /// The default implementation logs every error as a warning.
    fn on_error(&mut self, error: UnpackError<crate::messaging::NetMessage>) -> HandlerResult {
        warn!(
            self.log(),
            "Could not deserialise a message with Deserialiser with id={}. Error was: {:?}",
            Self::Deserialiser::SER_ID,
            error
        );
        Handled::OK
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
    fn receive_local(&mut self, msg: Self::Message) -> HandlerResult {
        self.receive(None, msg)
    }

    #[inline(always)]
    fn receive_network(&mut self, msg: crate::messaging::NetMessage) -> HandlerResult {
        match msg.try_into_deserialised::<_, <Self as NetworkActor>::Deserialiser>() {
            Ok(m) => self.receive(Some(m.sender), m.content),
            Err(e) => self.on_error(e),
        }
    }
}

use super::*;
use crate::{
    component::Handled,
    messaging::{DispatchEnvelope, MsgEnvelope, NetMessage, UnpackError},
};
use std::{
    fmt,
    sync::{Arc, Weak},
};

mod paths;
mod refs;
use crate::prelude::NetworkStatusPort;
pub use paths::*;
pub use refs::*;

/// Just a trait alias hack to avoid constantly writing `Debug+Send+'static`
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

/// The base trait for all actors
///
/// This trait handles raw message envelopes, without any unpacking
/// or other convenient syntactic sugars.
///
/// Usually it's better to use the unwrapped functions in [Actor](Actor),
/// but this can be more efficient at times, for example
/// when the content of the envelope isn't actually accessed in the component.
///
/// # Example
///
/// ```
/// use kompact::prelude::*;
///
/// #[derive(ComponentDefinition)]
/// struct RawActor {
///    ctx: ComponentContext<Self>
/// }
/// ignore_lifecycle!(RawActor);
/// impl ActorRaw for RawActor {
///     type Message = ();
///
///     fn receive(&mut self, env: MsgEnvelope<Self::Message>) -> Handled {
///         match env {
///            MsgEnvelope::Typed(m) => info!(self.log(), "Got a local message: {:?}", m),
///            MsgEnvelope::Net(nm) => info!(self.log(), "Got a network message: {:?}", nm),
///         }
///         Handled::Ok
///     }
/// }
/// ```
pub trait ActorRaw {
    /// The type of local messages the actor accepts
    type Message: MessageBounds;

    /// Handle an incoming message
    ///
    /// Incoming messages can either be local, in which case they are of type
    /// `Self::Message` (wrapped into a [MsgEnvelope](MsgEnvelope::Typed)),
    /// or they are coming from the network, in which case they of type
    /// [NetMessage](NetMessage) (again wrapped into a [MsgEnvelope](MsgEnvelope::Net)).
    ///
    /// # Note
    ///
    /// Remember that components usually run on a shared thread pool,
    /// so, just like for [handle](Provide::handle) implementations,
    /// you shouldn't ever block in this method unless you know what you are doing.
    fn receive(&mut self, env: MsgEnvelope<Self::Message>) -> Handled;
}

/// A slightly higher level Actor API that handles both local and networked messages
///
/// This trait should generally be preferred over [ActorRaw](ActorRaw), as it abstracts
/// away the message envelope enum used internally.
///
/// Actor can also be derived via `#[derive(Actor)]` for components which don't require
/// this messaging mechanism.
///
/// # Example
///
/// ```
/// use kompact::prelude::*;
///
/// #[derive(ComponentDefinition)]
/// struct NormalActor {
///    ctx: ComponentContext<Self>
/// }
/// ignore_lifecycle!(NormalActor);
/// impl Actor for NormalActor {
///     type Message = ();
///
///     fn receive_local(&mut self, msg: Self::Message) -> Handled {
///         info!(self.log(), "Got a local message: {:?}", msg);
///         Handled::Ok
///     }
///
///     fn receive_network(&mut self, msg: NetMessage) -> Handled {
///         info!(self.log(), "Got a network message: {:?}", msg);
///         Handled::Ok
///     }
/// }
/// ```
pub trait Actor {
    /// The type of local messages the actor accepts
    type Message: MessageBounds;

    /// Handle an incoming local message
    ///
    /// Local message are of type `Self::Message`.
    ///
    /// # Note
    ///
    /// Remember that components usually run on a shared thread pool,
    /// so, just like for [handle](Provide::handle) implementations,
    /// you shouldn't ever block in this method unless you know what you are doing.
    fn receive_local(&mut self, msg: Self::Message) -> Handled;

    /// Handle an incoming network message
    ///
    /// Network messages are of type [NetMessage](NetMessage) and
    /// can be either be serialised data, or a heap-allocated "reflected"
    /// message. Messages are "reflected" instead of serialised whenever possible
    /// for messages sent to an [ActorPath](ActorPath) that turned out to be in the same
    /// [KompactSystem](KompactSystem).
    ///
    /// # Note
    ///
    /// Remember that components usually run on a shared thread pool,
    /// so, just like for [handle](Provide::handle) implementations,
    /// you shouldn't ever block in this method unless you know what you are doing.
    fn receive_network(&mut self, msg: NetMessage) -> Handled;
}

/// A dispatcher is a system component that knows how to route messages and create system paths
///
/// If you need a custom networking implementation, it must implement `Dispatcher`
/// to allow messages to be routed correctly to channels, for example.
///
/// See [NetworkDispatcher](crate::prelude::NetworkDispatcher) for the provided networking dispatcher solution.
pub trait Dispatcher: ActorRaw<Message = DispatchEnvelope> {
    /// Returns the system path for this dispatcher
    fn system_path(&mut self) -> SystemPath;

    /// Connects the Dispatcher to the `RequiredPort<NetworkStatusPort>` reference.
    fn connect_network_status_port(&mut self, required: &mut RequiredPort<NetworkStatusPort>)
        -> ();
}

impl<A, M: MessageBounds> ActorRaw for A
where
    A: Actor<Message = M>,
{
    type Message = M;

    fn receive(&mut self, env: MsgEnvelope<M>) -> Handled {
        match env {
            MsgEnvelope::Typed(m) => self.receive_local(m),
            MsgEnvelope::Net(nm) => self.receive_network(nm),
        }
    }
}

/// A trait for things that have associated [actor references](ActorRef)
pub trait ActorRefFactory {
    /// The type of messages carried by references produced by this factory
    type Message: MessageBounds;

    /// Returns the associated actor reference
    fn actor_ref(&self) -> ActorRef<Self::Message>;
}

/// A trait for things that can deal with [network messages](NetMessage)
pub trait DynActorRefFactory {
    /// Returns a version of an actor ref that can only be used for [network messages](NetMessage),
    /// but not for typed messages
    fn dyn_ref(&self) -> DynActorRef;
}

/// A trait for accessing [dispatcher references](DispatcherRef)
pub trait Dispatching {
    /// Returns the associated dispatcher reference
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
///     fn receive(&mut self, sender: Option<ActorPath>, msg: Self::Message) -> Handled {
///         info!(self.log(), "Got a local or deserialised remote message: {:?}", msg);
///         Handled::Ok
///     }
/// }
/// ```
pub trait NetworkActor: ComponentLogging {
    /// The type of messages the actor accepts
    type Message: MessageBounds;
    /// The deserialiser used to unpack [network messages](NetMessage)
    /// into `Self::Message`.
    type Deserialiser: Deserialiser<Self::Message>;

    /// Handles all messages after deserialisation
    ///
    /// The `sender` argument will only be supplied if the original message
    /// was a [NetMessage](crate::messaging::NetMessage), otherwise it's `None`.
    ///
    /// All messages are of type `Self::Message`.
    ///
    /// # Note
    ///
    /// Remember that components usually run on a shared thread pool,
    /// so, just like for [handle](Provide::handle) implementations,
    /// you shouldn't ever block in this method unless you know what you are doing.
    fn receive(&mut self, sender: Option<ActorPath>, msg: Self::Message) -> Handled;

    /// Handle errors during unpacking of network messages
    ///
    /// The default implementation logs every error as a warning.
    fn on_error(&mut self, error: UnpackError<NetMessage>) -> Handled {
        warn!(
            self.log(),
            "Could not deserialise a message with Deserialiser with id={}. Error was: {:?}",
            Self::Deserialiser::SER_ID,
            error
        );
        Handled::Ok
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
    fn receive_local(&mut self, msg: Self::Message) -> Handled {
        self.receive(None, msg)
    }

    #[inline(always)]
    fn receive_network(&mut self, msg: NetMessage) -> Handled {
        match msg.try_into_deserialised::<_, <Self as NetworkActor>::Deserialiser>() {
            Ok(m) => self.receive(Some(m.sender), m.content),
            Err(e) => self.on_error(e),
        }
    }
}

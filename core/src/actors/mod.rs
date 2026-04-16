use super::*;
use crate::{
    component::Handled,
    messaging::{DispatchEnvelope, MsgEnvelope},
};
use std::{
    fmt,
    sync::{Arc, Weak},
};

#[cfg(feature = "distributed")]
mod distributed;
mod paths;
#[cfg(feature = "distributed")]
mod paths_dispatch;
mod refs;
#[cfg(feature = "distributed")]
pub use distributed::*;
pub use paths::*;
#[cfg(feature = "distributed")]
pub use paths_dispatch::*;
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

/// A slightly higher level Actor API that handles local messages, and optionally distributed ones
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
///     #[cfg(feature = "distributed")]
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

    #[cfg(feature = "distributed")]
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
    fn receive_network(&mut self, msg: crate::messaging::NetMessage) -> Handled;
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
}

impl<A, M: MessageBounds> ActorRaw for A
where
    A: Actor<Message = M>,
{
    type Message = M;

    fn receive(&mut self, env: MsgEnvelope<M>) -> Handled {
        match env {
            MsgEnvelope::Typed(m) => self.receive_local(m),
            #[cfg(feature = "distributed")]
            MsgEnvelope::Net(nm) => self.receive_network(nm),
            #[cfg(not(feature = "distributed"))]
            MsgEnvelope::Net(_nm) => {
                unreachable!("network messages require the `distributed` feature")
            }
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

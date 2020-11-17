use super::*;
use crate::routing::groups::StorePolicy;

/// An error that can occur during [actor path](ActorPath) registration
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum RegistrationError {
    /// An actor path with the same name exists already
    DuplicateEntry,
    /// This kind of registration is unsupported by the system's dispatcher implementation
    Unsupported,
    /// The supplied path was not invalid
    InvalidPath(PathParseError),
}

/// Convenience alias for the result of a path registration attempt
pub type RegistrationResult = Result<ActorPath, RegistrationError>;

/// A holder for different variants of how feedback for a registration can be provided
#[derive(Debug)]
pub enum RegistrationPromise {
    /// Provide feedback via fulfilling the promise
    Fulfil(utils::KPromise<RegistrationResult>),
    /// Do not provide feedback
    None,
}

/// An actor registration event
///
/// This is used for registering an [ActorRef](crate::prelude::ActorRef)
/// with an [ActorPath](crate::prelude::ActorPath) on a dispatcher.
#[derive(Debug)]
pub struct ActorRegistration {
    /// A network-only reference to the registered actor
    pub actor: DynActorRef,
    /// The path we want to register
    pub path: PathResolvable,
}

/// A routing policy registration event
///
/// This is used for registering an [RoutingPolicy](crate::routing::groups::RoutingPolicy)
/// (wrapped into a [StorePolicy](crate::routing::groups::RoutingPolicy) for abstraction)
/// with a named [ActorPath](crate::prelude::ActorPath) on a dispatcher.
#[derive(Debug)]
pub struct PolicyRegistration {
    /// A network-only reference to the registered actor
    pub policy: StorePolicy,
    /// The path we want to register
    pub path: Vec<String>,
}

/// One of the two registration event types
#[derive(Debug)]
pub enum RegistrationEvent {
    /// An actor registration event
    Actor(ActorRegistration),
    /// A routing policy registration event
    Policy(PolicyRegistration),
}
impl From<ActorRegistration> for RegistrationEvent {
    fn from(r: ActorRegistration) -> Self {
        RegistrationEvent::Actor(r)
    }
}
impl From<PolicyRegistration> for RegistrationEvent {
    fn from(r: PolicyRegistration) -> Self {
        RegistrationEvent::Policy(r)
    }
}

/// Envelope representing some registration event
///
/// Supported registration events are:
/// 1. Binding an actor reference to an actor path.
/// 2. Binding a routing policy to an named path.
#[derive(Debug)]
pub struct RegistrationEnvelope {
    /// The actual registration event
    pub event: RegistrationEvent,
    /// Allow existing registrations to be replaced by this registration
    ///
    /// If `false`, attempting to register an existing path will result in an error.
    pub update: bool,
    /// An optional feedback promise, which returns the newly registered actor path or an error
    pub promise: RegistrationPromise,
}

impl RegistrationEnvelope {
    /// Create an actor registration envelope without a promise for feedback
    pub fn actor(actor: DynActorRef, path: PathResolvable, update: bool) -> Self {
        let event = ActorRegistration { actor, path };
        RegistrationEnvelope {
            event: event.into(),
            update,
            promise: RegistrationPromise::None,
        }
    }

    /// Create an actor registration envelope using a promise for feedback
    pub fn actor_with_promise(
        actor: DynActorRef,
        path: PathResolvable,
        update: bool,
        promise: utils::KPromise<RegistrationResult>,
    ) -> Self {
        let event = ActorRegistration { actor, path };
        RegistrationEnvelope {
            event: event.into(),
            update,
            promise: RegistrationPromise::Fulfil(promise),
        }
    }

    /// Create a policy registration envelope without a promise for feedback
    pub fn policy<P>(policy: P, path: Vec<String>, update: bool) -> Self
    where
        P: Into<StorePolicy>,
    {
        let event = PolicyRegistration {
            policy: policy.into(),
            path,
        };
        RegistrationEnvelope {
            event: event.into(),
            update,
            promise: RegistrationPromise::None,
        }
    }

    /// Create a policy registration envelope without a promise for feedback
    pub fn policy_with_promise<P>(
        policy: P,
        path: Vec<String>,
        update: bool,
        promise: utils::KPromise<RegistrationResult>,
    ) -> Self
    where
        P: Into<StorePolicy>,
    {
        let event = PolicyRegistration {
            policy: policy.into(),
            path,
        };
        RegistrationEnvelope {
            event: event.into(),
            update,
            promise: RegistrationPromise::Fulfil(promise),
        }
    }
}

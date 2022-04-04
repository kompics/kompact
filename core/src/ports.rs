use std::{
    error::Error,
    fmt::{self, Debug},
    sync::{Arc, Weak},
};
use uuid::Uuid;

use super::*;

/// A Kompact port specifies the API of an abstraction
///
/// Such an API consists of *requests*, which are events handled by a provider component,
/// and *indications*, which are events triggered by providers and handled by components that require the abstraction.
///
/// All events on ports must be safe to send between threads, and must be cloneable,
/// as multiple components may be connected to a single port,
/// in which case each component will receive its own copy of the event.
/// For types that are expensive to clone, but read-only, the usage of an [Arc](std::sync::Arc) is recommended.
///
/// # Example
///
/// Consider an abstraction that sums up numbers it is given, and can be asked to provide the current total.
///
/// ```
/// use kompact::prelude::*;
///
/// #[derive(Clone, Debug)]
/// struct Sum(u64);
///
/// #[derive(Clone, Debug)]
/// enum SumRequest {
///     Number(u64),
///     Query   
/// }
///
/// struct SummerPort;
/// impl Port for SummerPort {
///     type Indication = Sum;
///     type Request = SumRequest;
/// }
/// ```
pub trait Port {
    /// Indications are triggered by provider components (see [Provide](Provide))
    /// and handled by requiring components (see [Require](Require)).
    type Indication: Sized + Send + 'static + Clone + Debug;
    /// Requests are triggered by requiring components (see [Require](Require))
    /// and handled by provider components (see [Provide](Provide)).
    type Request: Sized + Send + 'static + Clone + Debug;
}

struct CommonPortData<P: Port + 'static> {
    provide_channels: Vec<ProvidedRef<P>>,
    require_channels: Vec<RequiredRef<P>>,
}

impl<P: Port + 'static> CommonPortData<P> {
    fn new() -> CommonPortData<P> {
        CommonPortData {
            provide_channels: Vec::new(),
            require_channels: Vec::new(),
        }
    }

    fn clear_by_id(&mut self, component_id: Uuid) -> bool {
        let mut found_match = false;
        self.provide_channels.retain(|channel| {
            let channel_match = matches!(
                channel.owned_by_component_with_id(component_id),
                OwnershipResult::Owned | OwnershipResult::Deallocated
            );
            found_match |= channel_match;
            !channel_match
        });
        self.require_channels.retain(|channel| {
            let channel_match = matches!(
                channel.owned_by_component_with_id(component_id),
                OwnershipResult::Owned | OwnershipResult::Deallocated
            );
            found_match |= channel_match;
            !channel_match
        });

        found_match
    }

    fn cleanup(&mut self) {
        self.provide_channels.retain(|channel| channel.is_live());
        self.require_channels.retain(|channel| channel.is_live());
    }
}

/// An instance of a port type `P` that is marked as provided
///
/// Any component `C` that holds an instance of a provided port for `P`
/// *must* also implement [`Provide<P>`](Provide).
///
/// # Example
///
/// ```
/// use kompact::prelude::*;
///
///
/// struct UnitPort;
/// impl Port for UnitPort {
///     type Indication = ();
///     type Request = ();
/// }
///
/// #[derive(ComponentDefinition, Actor)]
/// struct UnitProvider {
///    ctx: ComponentContext<Self>,
///    unit_port: ProvidedPort<UnitPort>,
/// }
/// impl UnitProvider {
///     fn new() -> UnitProvider {
///         UnitProvider {
///             ctx: ComponentContext::uninitialised(),
///             unit_port: ProvidedPort::uninitialised(),
///         }
///     }    
/// }
/// ignore_lifecycle!(UnitProvider);
/// impl Provide<UnitPort> for UnitProvider {
///     fn handle(&mut self, event: ()) -> Handled {
///         Handled::Ok // handle event
///     }    
/// }
/// ```
pub struct ProvidedPort<P: Port + 'static> {
    common: CommonPortData<P>,
    parent: Option<Weak<dyn CoreContainer>>,
    msg_queue: Arc<ConcurrentQueue<P::Request>>,
}

impl<P: Port + 'static> ProvidedPort<P> {
    /// Create a new provided port for port type `P`
    ///
    /// # Note
    ///
    /// This port instance can only be used *after* the parent component has been created,
    /// and *not* during the constructor or anywhere else!
    pub fn uninitialised() -> ProvidedPort<P> {
        ProvidedPort {
            common: CommonPortData::new(),
            parent: None,
            msg_queue: Arc::new(ConcurrentQueue::new()),
        }
    }

    /// Trigger `event` on this port.
    ///
    /// Only *indication* events can be triggered on provided ports.
    ///
    /// Triggered events will be cloned for each connected channel.
    pub fn trigger(&mut self, event: P::Indication) -> () {
        let mut cleanup_required = false;
        self.common
            .require_channels
            .iter()
            .for_each_with(event, |c, e| {
                cleanup_required |= c.enqueue(e);
            });
        if cleanup_required {
            self.common.cleanup();
        }
    }

    /// Connect to a required port reference
    ///
    /// Port references are acquired via the [share](RequiredPort::share) function.
    pub fn connect(&mut self, c: RequiredRef<P>) -> () {
        self.common.require_channels.push(c);
    }

    /// Remove the given port from our connection list
    ///
    /// Returns `true` if the connections changed,
    /// i.e. when `c` was present.
    pub fn disconnect_port(&mut self, c: RequiredRef<P>) -> bool {
        let mut found_match = false;
        self.common.require_channels.retain(|other| {
            let matches = &c == other;
            found_match |= matches;
            !matches
        });
        found_match
    }

    /// Removes all ports owned by component `c` from our connection list
    ///
    /// Returns `true` if the connections changed,
    /// i.e. when at least one port was owned by `c`.
    pub fn disconnect_component(&mut self, c: &dyn CoreContainer) -> bool {
        self.common.clear_by_id(c.id())
    }

    /// Share a reference to this port to connect to
    ///
    /// Ports are connected to references via the [connect](RequiredPort::connect) function.
    pub fn share(&mut self) -> ProvidedRef<P> {
        match self.parent {
            Some(ref p) => {
                let core_container = p.clone();
                ProvidedRef {
                    msg_queue: Arc::downgrade(&self.msg_queue),
                    component: core_container,
                }
            }
            None => panic!("Port is not properly initialized!"),
        }
    }

    /// Mark `p` as the parent component of this port
    ///
    /// This method should only be used in custom [ComponentDefinition](ComponentDefinition) implementations!
    pub fn set_parent(&mut self, p: Arc<dyn CoreContainer>) -> () {
        self.parent = Some(Arc::downgrade(&p));
    }

    /// Take the first element off the queue, if any
    ///
    /// This method should only be used in custom [ComponentDefinition](ComponentDefinition) implementations!
    pub fn dequeue(&self) -> Option<P::Request> {
        self.msg_queue.pop().ok()
    }
}

/// An instance of a port type `P` that is marked as required
///
/// Any component `C` that holds an instance of a required port for `P`
/// *must* also implement [`Require<P>`](Require).
///
/// # Example
///
/// ```
/// use kompact::prelude::*;
///
///
/// struct UnitPort;
/// impl Port for UnitPort {
///     type Indication = ();
///     type Request = ();
/// }
///
/// #[derive(ComponentDefinition, Actor)]
/// struct UnitRequirer {
///    ctx: ComponentContext<Self>,
///    unit_port: RequiredPort<UnitPort>,
/// }
/// impl UnitRequirer {
///     fn new() -> UnitRequirer {
///         UnitRequirer {
///             ctx: ComponentContext::uninitialised(),
///             unit_port: RequiredPort::uninitialised(),
///         }
///     }    
/// }
/// ignore_lifecycle!(UnitRequirer);
/// impl Require<UnitPort> for UnitRequirer {
///     fn handle(&mut self, event: ()) -> Handled {
///         Handled::Ok // handle event
///     }    
/// }
/// ```
pub struct RequiredPort<P: Port + 'static> {
    common: CommonPortData<P>,
    parent: Option<Weak<dyn CoreContainer>>,
    msg_queue: Arc<ConcurrentQueue<P::Indication>>,
}

impl<P: Port + 'static> RequiredPort<P> {
    /// Create a new required port for port type `P`
    ///
    /// # Note
    ///
    /// This port instance can only be used *after* the parent component has been created,
    /// and *not* during the constructor or anywhere else!
    pub fn uninitialised() -> RequiredPort<P> {
        RequiredPort {
            common: CommonPortData::new(),
            parent: None,
            msg_queue: Arc::new(ConcurrentQueue::new()),
        }
    }

    /// Trigger `event` on this port.
    ///
    /// Only *requests* events can be triggered on required ports.
    ///
    /// Triggered events will be cloned for each connected channel.
    pub fn trigger(&mut self, event: P::Request) -> () {
        let mut cleanup_required = false;
        self.common
            .provide_channels
            .iter()
            .for_each_with(event, |c, e| {
                cleanup_required |= c.enqueue(e);
            });
        if cleanup_required {
            self.common.cleanup();
        }
    }

    /// Connect to a provided port reference
    ///
    /// Port references are acquired via the [share](ProvidedPort::share) function.
    pub fn connect(&mut self, c: ProvidedRef<P>) -> () {
        self.common.provide_channels.push(c);
    }

    /// Remove the given port from our connection list
    ///
    /// Returns `true` if the connections changed,
    /// i.e. when `c` was present.
    pub fn disconnect_port(&mut self, c: ProvidedRef<P>) -> bool {
        let mut found_match = false;
        self.common.provide_channels.retain(|other| {
            let matches = &c == other;
            found_match |= matches;
            !matches
        });
        found_match
    }

    /// Removes all ports owned by component `c` from our connection list
    ///
    /// Returns `true` if the connections changed,
    /// i.e. when at least one port was owned by `c`.
    pub fn disconnect_component(&mut self, c: &dyn CoreContainer) -> bool {
        self.common.clear_by_id(c.id())
    }

    /// Share a reference to this port to connect to
    ///
    /// Ports are connected to references via the [connect](ProvidedPort::connect) function.
    pub fn share(&mut self) -> RequiredRef<P> {
        match self.parent {
            Some(ref p) => {
                let core_container = p.clone();
                RequiredRef {
                    msg_queue: Arc::downgrade(&self.msg_queue),
                    component: core_container,
                }
            }
            None => panic!("Port is not properly initialized!"),
        }
    }

    /// Mark `p` as the parent component of this port
    ///
    /// This method should only be used in custom [ComponentDefinition](ComponentDefinition) implementations!
    pub fn set_parent(&mut self, p: Arc<dyn CoreContainer>) -> () {
        self.parent = Some(Arc::downgrade(&p));
    }

    /// Take the first element off the queue, if any
    ///
    /// This method should only be used in custom [ComponentDefinition](ComponentDefinition) implementations!
    pub fn dequeue(&self) -> Option<P::Indication> {
        self.msg_queue.pop().ok()
    }
}

/// A reference to a provided port
///
/// Can be connected to a [RequiredPort](RequiredPort::connect).
pub struct ProvidedRef<P: Port + 'static> {
    component: Weak<dyn CoreContainer>,
    msg_queue: Weak<ConcurrentQueue<P::Request>>,
}

impl<P: Port + 'static> Clone for ProvidedRef<P> {
    fn clone(&self) -> ProvidedRef<P> {
        ProvidedRef {
            component: self.component.clone(),
            msg_queue: self.msg_queue.clone(),
        }
    }
}

impl<P: Port + 'static> PartialEq for ProvidedRef<P> {
    fn eq(&self, other: &Self) -> bool {
        // only compare message queue, as this is more specific than component
        self.msg_queue.ptr_eq(&other.msg_queue)
    }
}

impl<P: Port + 'static> ProvidedRef<P> {
    /// Returns `true` if there are deallocated ports
    pub(crate) fn enqueue(&self, event: P::Request) -> bool {
        match (self.msg_queue.upgrade(), self.component.upgrade()) {
            (Some(q), Some(c)) => {
                let sd = c.core().increment_work();
                q.push(event);
                //println!("PROVIDED REF");
                if let SchedulingDecision::Schedule = sd {
                    let system = c.core().system();
                    system.schedule(c.clone());
                }
                false
            }
            (_q, _c) => {
                #[cfg(test)]
                println!(
                    "Dropping event as target (queue? {:?}, component? {:?}) is unavailable: {:?}",
                    _q.is_some(),
                    _c.is_some(),
                    event
                );
                true
            }
        }
    }

    fn is_live(&self) -> bool {
        self.component.strong_count() > 0 && self.msg_queue.strong_count() > 0
    }

    /// Checks if the component that owns this port has id == `component_id`
    ///
    /// Returns `OwnershipResult::Owned` if the above statement is true,
    /// and `OwnershipResult::NotOwned`, if it is false.
    /// Also returns `OwnershipResult::Deallocated` if the component was already deallocated.
    pub fn owned_by_component_with_id(&self, component_id: Uuid) -> OwnershipResult {
        if let Some(c) = self.component.upgrade() {
            if c.id() == component_id {
                OwnershipResult::Owned
            } else {
                OwnershipResult::NotOwned
            }
        } else {
            OwnershipResult::Deallocated
        }
    }
}

/// A reference to a required port
///
/// Can be connected to a [ProvidedPort](ProvidedPort::connect).
pub struct RequiredRef<P: Port + 'static> {
    component: Weak<dyn CoreContainer>,
    msg_queue: Weak<ConcurrentQueue<P::Indication>>,
}

impl<P: Port + 'static> Clone for RequiredRef<P> {
    fn clone(&self) -> RequiredRef<P> {
        RequiredRef {
            component: self.component.clone(),
            msg_queue: self.msg_queue.clone(),
        }
    }
}

impl<P: Port + 'static> PartialEq for RequiredRef<P> {
    fn eq(&self, other: &Self) -> bool {
        // only compare message queue, as this is more specific than component
        self.msg_queue.ptr_eq(&other.msg_queue)
    }
}

impl<P: Port + 'static> RequiredRef<P> {
    /// Returns `true` if there are deallocated ports
    pub(crate) fn enqueue(&self, event: P::Indication) -> bool {
        match (self.msg_queue.upgrade(), self.component.upgrade()) {
            (Some(q), Some(c)) => {
                let sd = c.core().increment_work();
                q.push(event);
                //println!("REQUIRED REF");
                if let SchedulingDecision::Schedule = sd {
                    let system = c.core().system();
                    system.schedule(c.clone());
                }
                false
            }
            (_q, _c) => {
                #[cfg(test)]
                println!(
                    "Dropping event as target (queue? {:?}, component? {:?}) is unavailable: {:?}",
                    _q.is_some(),
                    _c.is_some(),
                    event
                );
                true
            }
        }
    }

    fn is_live(&self) -> bool {
        self.component.strong_count() > 0 && self.msg_queue.strong_count() > 0
    }

    /// Checks if the component that owns this port has id == `component_id`
    ///
    /// Returns `OwnershipResult::Owned` if the above statement is true,
    /// and `OwnershipResult::NotOwned`, if it is false.
    /// Also returns `OwnershipResult::Deallocated` if the component was already deallocated.
    pub fn owned_by_component_with_id(&self, component_id: Uuid) -> OwnershipResult {
        if let Some(c) = self.component.upgrade() {
            if c.id() == component_id {
                OwnershipResult::Owned
            } else {
                OwnershipResult::NotOwned
            }
        } else {
            OwnershipResult::Deallocated
        }
    }
}

/// Result of checking port ownershipt
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnershipResult {
    /// The given port was owned by the checked component
    Owned,
    /// The given port was *not* owned by the checked component
    NotOwned,
    /// The component was already deallocated,
    /// so no comparison could be performed
    Deallocated,
}

/// A non-generic version of [sync::TryLockError](std::sync::TryLockError)
#[derive(Debug, PartialEq, Eq)]
pub enum TryLockError {
    /// Some mutex is poisoned
    Poisoned,
    /// Some mutex would block
    WouldBlock,
}
impl fmt::Display for TryLockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(
            &match *self {
                TryLockError::Poisoned => "poisoned lock: another task failed inside",
                TryLockError::WouldBlock => "try_lock failed because the operation would block",
            },
            f,
        )
    }
}
impl Error for TryLockError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        None
    }
}
impl<T> From<std::sync::TryLockError<T>> for TryLockError {
    fn from(error: std::sync::TryLockError<T>) -> Self {
        match error {
            std::sync::TryLockError::Poisoned(_) => TryLockError::Poisoned,
            std::sync::TryLockError::WouldBlock => TryLockError::WouldBlock,
        }
    }
}

/// Errors that can occur when trying to disconnect a channel
pub struct DisconnectError<C> {
    /// The original channel
    pub channel: C,
    /// Description of the error that occurred
    pub error: TryLockError,
}

impl<C> fmt::Debug for DisconnectError<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DisconnectError")
            .field("error", &self.error)
            .finish()
    }
}

impl<C> fmt::Display for DisconnectError<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "channel disconnection failed")
    }
}

impl<C> Error for DisconnectError<C> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.error)
    }
}

trait InternalChannel {
    fn disconnect_impl(&self) -> Result<(), TryLockError>;
}

/// Common functionality for all channel types
pub trait Channel {
    /// Gives a type erased version of this channel
    fn boxed(self) -> Box<dyn Channel + Send + 'static>
    where
        Self: Sized;

    /// Disconnects this channel
    ///
    /// # Note
    ///
    /// Disconnecting channels requires the mutex
    /// if all involved components to be locked,
    /// and can thus interfere with
    /// execution or even lead to deadlocks.
    ///
    /// It is recommended to only attempt to disconnect
    /// channels from components that are paused or killed.
    fn disconnect(self) -> Result<(), DisconnectError<Self>>
    where
        Self: Sized,
    {
        let res = self.disconnect_by_ref();
        res.map_err(|e| DisconnectError {
            channel: self,
            error: e,
        })
    }

    /// Disconnects this channel without consuming it
    ///
    /// # Usage Note
    ///
    /// A channel is useless after being disconnected,
    /// so it is recommended to use [disconnect](Self::disconnect)
    /// where possible instead.
    ///
    /// # Note
    ///
    /// Disconnecting channels requires the mutex
    /// if all involved components to be locked,
    /// and can thus interfere with
    /// execution or even lead to deadlocks.
    ///
    /// It is recommended to only attempt to disconnect
    /// channels from components that are paused or killed.
    fn disconnect_by_ref(&self) -> Result<(), TryLockError>;
}

/// The channel resulting from a call to [biconnect_components](crate::biconnect_components)
pub struct TwoWayChannel<P, C1, C2>
where
    P: Port + 'static,
    C1: ComponentDefinition + Sized + 'static + Provide<P> + ProvideRef<P>,
    C2: ComponentDefinition + Sized + 'static + Require<P> + RequireRef<P>,
{
    provider: Weak<Component<C1>>,
    requirer: Weak<Component<C2>>,
    provided_ref: ProvidedRef<P>,
    required_ref: RequiredRef<P>,
}

impl<P, C1, C2> TwoWayChannel<P, C1, C2>
where
    P: Port + 'static,
    C1: ComponentDefinition + Sized + 'static + Provide<P> + ProvideRef<P>,
    C2: ComponentDefinition + Sized + 'static + Require<P> + RequireRef<P>,
{
    pub(crate) fn new(
        provider: &Arc<Component<C1>>,
        requirer: &Arc<Component<C2>>,
        provided_ref: ProvidedRef<P>,
        required_ref: RequiredRef<P>,
    ) -> Self {
        TwoWayChannel {
            provider: Arc::downgrade(provider),
            requirer: Arc::downgrade(requirer),
            provided_ref,
            required_ref,
        }
    }
}
impl<P, C1, C2> Channel for TwoWayChannel<P, C1, C2>
where
    P: Port + 'static,
    C1: ComponentDefinition + Sized + 'static + Provide<P> + ProvideRef<P>,
    C2: ComponentDefinition + Sized + 'static + Require<P> + RequireRef<P>,
{
    fn boxed(self) -> std::boxed::Box<(dyn Channel + Send + 'static)> {
        Box::new(self)
    }

    fn disconnect_by_ref(&self) -> Result<(), TryLockError> {
        if let Some(component) = self.provider.upgrade() {
            let mut core = component.mutable_core.try_lock()?;
            ProvideRef::disconnect(&mut core.definition, self.required_ref.clone());
        }
        if let Some(component) = self.requirer.upgrade() {
            let mut core = component.mutable_core.try_lock()?;
            RequireRef::disconnect(&mut core.definition, self.provided_ref.clone());
        }
        Ok(())
    }
}

/// The channel resulting from a call to [connect_to_required](crate::ProvideRef::connect_to_required)
pub struct ProviderChannel<P, C>
where
    P: Port + 'static,
    C: ComponentDefinition + Sized + 'static + Provide<P> + ProvideRef<P>,
{
    provider: Weak<Component<C>>,
    required_ref: RequiredRef<P>,
}

impl<P, C> ProviderChannel<P, C>
where
    P: Port + 'static,
    C: ComponentDefinition + Sized + 'static + Provide<P> + ProvideRef<P>,
{
    pub(crate) fn new(provider: &Arc<Component<C>>, required_ref: RequiredRef<P>) -> Self {
        ProviderChannel {
            provider: Arc::downgrade(provider),
            required_ref,
        }
    }
}

impl<P, C> Channel for ProviderChannel<P, C>
where
    P: Port + 'static,
    C: ComponentDefinition + Sized + 'static + Provide<P> + ProvideRef<P>,
{
    fn boxed(self) -> std::boxed::Box<(dyn Channel + Send + 'static)> {
        Box::new(self)
    }

    fn disconnect_by_ref(&self) -> Result<(), TryLockError> {
        if let Some(component) = self.provider.upgrade() {
            let mut core = component.mutable_core.try_lock()?;
            ProvideRef::disconnect(&mut core.definition, self.required_ref.clone());
        }
        Ok(())
    }
}

/// The channel resulting from a call to [connect_to_provided](crate::RequireRef::connect_to_provided)
pub struct RequirerChannel<P, C>
where
    P: Port + 'static,
    C: ComponentDefinition + Sized + 'static + Require<P> + RequireRef<P>,
{
    requirer: Weak<Component<C>>,
    provided_ref: ProvidedRef<P>,
}

impl<P, C> RequirerChannel<P, C>
where
    P: Port + 'static,
    C: ComponentDefinition + Sized + 'static + Require<P> + RequireRef<P>,
{
    pub(crate) fn new(requirer: &Arc<Component<C>>, provided_ref: ProvidedRef<P>) -> Self {
        RequirerChannel {
            requirer: Arc::downgrade(requirer),
            provided_ref,
        }
    }
}

impl<P, C> Channel for RequirerChannel<P, C>
where
    P: Port + 'static,
    C: ComponentDefinition + Sized + 'static + Require<P> + RequireRef<P>,
{
    fn boxed(self) -> std::boxed::Box<(dyn Channel + Send + 'static)> {
        Box::new(self)
    }

    fn disconnect_by_ref(&self) -> Result<(), TryLockError> {
        if let Some(component) = self.requirer.upgrade() {
            let mut core = component.mutable_core.try_lock()?;
            RequireRef::disconnect(&mut core.definition, self.provided_ref.clone());
        }
        Ok(())
    }
}

impl Channel for Box<(dyn Channel + Send + 'static)> {
    fn boxed(self) -> std::boxed::Box<(dyn Channel + Send + 'static)> {
        self
    }

    fn disconnect_by_ref(&self) -> Result<(), TryLockError> {
        Channel::disconnect_by_ref(self.as_ref())
    }
}

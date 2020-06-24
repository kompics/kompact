use std::{
    fmt::Debug,
    sync::{Arc, Weak},
};

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
/// ignore_control!(UnitProvider);
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
        self.common
            .require_channels
            .iter()
            .for_each_with(event, |c, e| {
                c.enqueue(e);
            })
    }

    /// Connect to a required port reference
    ///
    /// Port references are acquired via the [share](RequiredPort::share) function.
    pub fn connect(&mut self, c: RequiredRef<P>) -> () {
        self.common.require_channels.push(c);
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
/// ignore_control!(UnitRequirer);
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
        self.common
            .provide_channels
            .iter()
            .for_each_with(event, |c, e| {
                c.enqueue(e);
            })
    }

    /// Connect to a provided port reference
    ///
    /// Port references are acquired via the [share](ProvidedPort::share) function.
    pub fn connect(&mut self, c: ProvidedRef<P>) -> () {
        self.common.provide_channels.push(c);
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

impl<P: Port + 'static> ProvidedRef<P> {
    pub(crate) fn new(
        component: Weak<dyn CoreContainer>,
        msg_queue: Weak<ConcurrentQueue<P::Request>>,
    ) -> ProvidedRef<P> {
        ProvidedRef {
            component,
            msg_queue,
        }
    }

    pub(crate) fn enqueue(&self, event: P::Request) -> () {
        match (self.msg_queue.upgrade(), self.component.upgrade()) {
            (Some(q), Some(c)) => {
                let sd = c.core().increment_work();
                q.push(event);
                match sd {
                    SchedulingDecision::Schedule => {
                        let system = c.core().system();
                        system.schedule(c.clone());
                    }
                    _ => (), // nothing
                }
            }
            (_q, _c) =>
            {
                #[cfg(test)]
                println!(
                    "Dropping event as target (queue? {:?}, component? {:?}) is unavailable: {:?}",
                    _q.is_some(),
                    _c.is_some(),
                    event
                )
            }
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

impl<P: Port + 'static> RequiredRef<P> {
    pub(crate) fn enqueue(&self, event: P::Indication) -> () {
        match (self.msg_queue.upgrade(), self.component.upgrade()) {
            (Some(q), Some(c)) => {
                let sd = c.core().increment_work();
                q.push(event);
                match sd {
                    SchedulingDecision::Schedule => {
                        let system = c.core().system();
                        system.schedule(c.clone());
                    }
                    _ => (), // nothing
                }
            }
            (_q, _c) =>
            {
                #[cfg(test)]
                println!(
                    "Dropping event as target (queue? {:?}, component? {:?}) is unavailable: {:?}",
                    _q.is_some(),
                    _c.is_some(),
                    event
                )
            }
        }
    }
}

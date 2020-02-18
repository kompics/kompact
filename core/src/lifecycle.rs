use super::*;

use std::sync::atomic::{AtomicUsize, Ordering};

/// A Kompact lifecycle event
///
/// Lifecycle events are produced by the Kompact system in response to certain API calls,
/// such as [start](KompactSystem::start), for example.
///
/// Lifecyle events are handled in a [ControlPort](ControlPort) handler, which is required for every component.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ControlEvent {
    /// Starts a component
    Start,
    /// Stops (pauses) a component
    Stop,
    /// Stops and deallocates a component
    Kill,
}

/// Kompact lifecycle port type
///
/// The port only has requests associated with it, and they are lifecyle events of type [ControlEvent](ControlEvent).
///
/// Every Kompact component *must* implement `Provide<ControlPort>`.
///
/// If no custom handling of lifecycle events is needed for a component,
/// the [ignore_control](ignore_control!) macro can be used instead.
///
/// Lifecycle events are produced and triggered on instances of this port by the Kompact system
/// in response to certain API calls, such as [start](KompactSystem::start), for example.
///
/// # Example
///
/// ```
/// use kompact::prelude::*;
///
/// #[derive(ComponentDefinition, Actor)]
/// struct TestComponent {
///     ctx: ComponentContext<TestComponent>,
/// }
/// impl Provide<ControlPort> for TestComponent {
///     fn handle(&mut self, event: ControlEvent) -> () {
///         match event {
///             ControlEvent::Start => (), // handle start event
///             ControlEvent::Stop => (), // handle stop event
///             ControlEvent::Kill => (), // handle kill event
///         }
///     }
/// }
/// ```
pub struct ControlPort;

impl Port for ControlPort {
    type Indication = Never;
    type Request = ControlEvent;
}

const PASSIVE: usize = 0;
const ACTIVE: usize = 1;
const DESTROYED: usize = 2;
const FAULTY: usize = 3;

pub(crate) fn initial_state() -> AtomicUsize {
    AtomicUsize::new(PASSIVE)
}

pub(crate) fn set_active(state: &AtomicUsize) {
    state.store(ACTIVE, Ordering::SeqCst);
}

pub(crate) fn set_passive(state: &AtomicUsize) {
    state.store(PASSIVE, Ordering::SeqCst);
}

pub(crate) fn set_destroyed(state: &AtomicUsize) {
    state.store(DESTROYED, Ordering::SeqCst);
}

pub(crate) fn set_faulty(state: &AtomicUsize) {
    state.store(FAULTY, Ordering::SeqCst);
}

pub(crate) fn is_active(state: &AtomicUsize) -> bool {
    state.load(Ordering::SeqCst) == ACTIVE
}

pub(crate) fn is_faulty(state: &AtomicUsize) -> bool {
    state.load(Ordering::SeqCst) == FAULTY
}

pub(crate) fn is_destroyed(state: &AtomicUsize) -> bool {
    state.load(Ordering::SeqCst) == DESTROYED
}

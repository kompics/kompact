use super::*;

/// The core of a Kompact component
///
/// Contains the unique id, as well as references to the Kompact system,
/// internal state variables, and the component instance itself.
pub struct ComponentCore {
    pub(super) id: Uuid,
    pub(super) system: KompactSystem,
    pub(super) state: AtomicU64,
    component: UnsafeCell<Weak<dyn CoreContainer>>,
}

impl ComponentCore {
    pub(crate) fn with<CC: CoreContainer + Sized + 'static>(
        system: KompactSystem,
    ) -> ComponentCore {
        let weak_sized = Weak::<CC>::new();
        let weak = weak_sized as Weak<dyn CoreContainer>;
        ComponentCore {
            id: Uuid::new_v4(),
            system,
            state: lifecycle::initial_state(),
            component: UnsafeCell::new(weak),
        }
    }

    pub(super) fn load_state(&self) -> LifecycleState {
        LifecycleState::load(&self.state)
    }

    /// Returns a reference to the Kompact system this component is a part of
    pub fn system(&self) -> &KompactSystem {
        &self.system
    }

    /// Returns the component's unique id
    pub fn id(&self) -> &Uuid {
        &self.id
    }

    pub(crate) unsafe fn set_component(&self, c: Arc<dyn CoreContainer>) -> () {
        let component_mut = self.component.get();
        *component_mut = Arc::downgrade(&c);
    }

    /// Returns the component instance itself, wrapped in an [Arc](std::sync::Arc)
    ///
    /// This method will panic if the component hasn't been properly initialised, yet!
    pub fn component(&self) -> Arc<dyn CoreContainer> {
        unsafe {
            match (*self.component.get()).upgrade() {
                Some(ac) => ac,
                None => panic!("Component already deallocated (or not initialised)!"),
            }
        }
    }

    pub(crate) fn increment_work(&self) -> SchedulingDecision {
        LifecycleState::increment_work(&self.state)
    }

    pub(super) fn decrement_work(&self, work_done: usize) -> SchedulingDecision {
        LifecycleState::decrement_work(&self.state, work_done)
    }

    pub(super) fn get_scheduling_decision(&self) -> SchedulingDecision {
        LifecycleState::load(&self.state).into_scheduling_decision()
    }
}

// The compiler gets stuck into a recursive loop trying to figure this out itself
//unsafe impl<C: ComponentDefinition + Sized> Send for Component<C> {}
//unsafe impl<C: ComponentDefinition + Sized> Sync for Component<C> {}

unsafe impl Send for ComponentCore {}

unsafe impl Sync for ComponentCore {}

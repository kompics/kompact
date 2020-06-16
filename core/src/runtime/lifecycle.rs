use std::sync::atomic::{AtomicUsize, Ordering};

const ACTIVE: usize = 0;
const DESTROYED: usize = 1;
const FAULTY: usize = 2;

pub(crate) fn initial_state() -> AtomicUsize {
    AtomicUsize::new(ACTIVE)
}

pub(crate) fn set_active(state: &AtomicUsize) {
    state.store(ACTIVE, Ordering::SeqCst);
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

use super::*;

use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ControlEvent {
    Start,
    Stop,
    Kill,
}

pub struct ControlPort;

impl Port for ControlPort {
    type Indication = ();
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

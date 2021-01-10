use super::*;

use std::sync::atomic::{AtomicU64, Ordering};

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
    /// Ask the component to poll a non-blocking future
    Poll(Uuid),
}

const ACTIVE: u64 = 0u64;
const PASSIVE: u64 = 1u64 << 61;
const DESTROYED: u64 = 2u64 << 61;
const FAULTY: u64 = 3u64 << 61;
const BLOCKING: u64 = 4u64 << 61;

const FLAG_MASK: u64 = u64::MAX >> 3;

pub(crate) enum LifecycleState {
    Active(u64),
    Passive(u64),
    Destroyed,
    Faulty,
    Blocking,
}
impl LifecycleState {
    fn from_current(current_state: u64) -> Self {
        let flags = remove_count(current_state);
        match flags {
            ACTIVE => LifecycleState::Active(remove_flags(current_state)),
            PASSIVE => LifecycleState::Passive(remove_flags(current_state)),
            DESTROYED => LifecycleState::Destroyed,
            FAULTY => LifecycleState::Faulty,
            BLOCKING => LifecycleState::Blocking,
            x => unreachable!("A component's state has become invalid! {:064b}", x),
        }
    }

    pub(crate) fn load(state: &AtomicU64) -> Self {
        let current_state = state.load(Ordering::SeqCst);
        Self::from_current(current_state)
    }

    pub(crate) fn into_scheduling_decision(self) -> SchedulingDecision {
        match self {
            LifecycleState::Active(work_count) => {
                if work_count == 0 {
                    SchedulingDecision::Schedule
                } else {
                    SchedulingDecision::AlreadyScheduled
                }
            }
            LifecycleState::Passive(work_count) => {
                if work_count == 0 {
                    SchedulingDecision::Schedule
                } else {
                    SchedulingDecision::AlreadyScheduled
                }
            }
            LifecycleState::Destroyed | LifecycleState::Faulty | LifecycleState::Blocking => {
                SchedulingDecision::NoWork
            }
        }
    }

    pub(crate) fn increment_work(state: &AtomicU64) -> SchedulingDecision {
        let old_state = state.fetch_add(1, Ordering::SeqCst);
        validate_add(old_state);
        Self::from_current(old_state).into_scheduling_decision()
    }

    pub(crate) fn decrement_work(state: &AtomicU64, work_done: usize) -> SchedulingDecision {
        let work_done = work_done as u64;
        let old_state = state.fetch_sub(work_done, Ordering::SeqCst);
        let old_count = remove_flags(old_state);
        match old_count.checked_sub(work_done) {
            Some(new_count) => {
                if new_count > 0 {
                    SchedulingDecision::Schedule
                } else {
                    SchedulingDecision::NoWork
                }
            }
            None => {
                eprintln!(
                    "Aborting process due to unrecoverable violated invariant: work_count >= 0"
                );
                std::process::abort();
            }
        }
    }
}

fn remove_flags(number: u64) -> u64 {
    number & FLAG_MASK
}
fn remove_count(number: u64) -> u64 {
    number & !FLAG_MASK
}

fn is_valid_add(old_number: u64) -> bool {
    let no_flags = remove_flags(old_number);
    no_flags < FLAG_MASK // FLAG_MASK is the same as maximum work count
}

fn validate_add(old_number: u64) {
    if !is_valid_add(old_number) {
        eprintln!(
            "Aborting process due to unrecoverable violated invariant: work_count < u64::MAX >> 3"
        );
        std::process::abort();
    }
}

pub(crate) fn initial_state() -> AtomicU64 {
    AtomicU64::new(PASSIVE)
}

fn set_state(state: &AtomicU64, new_state: u64) {
    loop {
        let current_state = state.load(Ordering::SeqCst);
        let current_count = remove_flags(current_state);
        let new_state = new_state | current_count;
        if state.compare_exchange(current_state, new_state, Ordering::SeqCst, Ordering::SeqCst)
            == Ok(current_state)
        {
            return;
        }
    }
}

pub(crate) fn set_active(state: &AtomicU64) {
    set_state(state, ACTIVE);
}

pub(crate) fn set_passive(state: &AtomicU64) {
    set_state(state, PASSIVE);
}

pub(crate) fn set_destroyed(state: &AtomicU64) {
    set_state(state, DESTROYED);
}

pub(crate) fn set_faulty(state: &AtomicU64) {
    set_state(state, FAULTY);
}

pub(crate) fn set_blocking(state: &AtomicU64) {
    set_state(state, BLOCKING);
}

pub(crate) fn is_active(state: &AtomicU64) -> bool {
    let current_state = state.load(Ordering::SeqCst);
    remove_count(current_state) == ACTIVE
}

pub(crate) fn is_faulty(state: &AtomicU64) -> bool {
    let current_state = state.load(Ordering::SeqCst);
    remove_count(current_state) == FAULTY
}

pub(crate) fn is_destroyed(state: &AtomicU64) -> bool {
    let current_state = state.load(Ordering::SeqCst);
    remove_count(current_state) == DESTROYED
}

#[allow(dead_code)]
pub(crate) fn is_blocking(state: &AtomicU64) -> bool {
    let current_state = state.load(Ordering::SeqCst);
    remove_count(current_state) == BLOCKING
}

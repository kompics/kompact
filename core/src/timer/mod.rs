use uuid::Uuid;

use hierarchical_hash_wheel_timer::{
    thread_timer::{TimerRef as GenericTimerRef, TimerWithThread as GenericTimerWithThread},
    simulation::{SimulationTimer as GenericSimulationTimer},
    OneshotState,
    PeriodicState,
    TimerEntry as GenericTimerEntry,
    TimerReturn as GenericTimerReturn,
    Timer as LowlevelTimer,
    OneShotClosureState, 
    PeriodicClosureState,
};

pub use hierarchical_hash_wheel_timer::TimerError;

pub(crate) mod timer_manager;
use timer_manager::{Timeout, TimerActorRef};

/// Indicate whether or not to reschedule a periodic timer
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TimerReturn {
    /// Reschedule the timer
    Reschedule,
    /// Do not reschedule the timer
    Cancel,
}
impl TimerReturn {
    /// Whether or not this timer should be rescheduled
    pub fn should_reschedule(&self) -> bool {
        match self {
            TimerReturn::Reschedule => true,
            TimerReturn::Cancel => false,
        }
    }
}
impl Default for TimerReturn {
    fn default() -> Self {
        TimerReturn::Reschedule
    }
}
impl From<TimerReturn> for GenericTimerReturn<()> {
    fn from(tr: TimerReturn) -> Self {
        match tr {
            TimerReturn::Reschedule => GenericTimerReturn::Reschedule(()),
            TimerReturn::Cancel => GenericTimerReturn::Cancel,
        }
    }
}

/// A concrete entry for an outstanding timeout
pub type TimerEntry = GenericTimerEntry<Uuid, ActorRefState, ActorRefState>;

/// The reference type for the timer thread
pub type TimerRef = GenericTimerRef<Uuid, ActorRefState, ActorRefState>;

/// The concrete vairant of timer thread used in Kompact
pub type TimerWithThread = GenericTimerWithThread<Uuid, ActorRefState, ActorRefState>;

// The concrete variant of the simulation timer
pub type SimulationTimer = GenericSimulationTimer<Uuid, ActorRefState, ActorRefState>;
pub type TimerCore = dyn LowlevelTimer<Id = Uuid, OneshotState = ActorRefState, PeriodicState = ActorRefState>;

/// The necessary state for Kompact timers
#[derive(Debug)]
pub struct ActorRefState {
    id: Uuid,
    receiver: TimerActorRef,
}

impl ActorRefState {
    fn new(id: Uuid, receiver: TimerActorRef) -> Self {
        ActorRefState { id, receiver }
    }
}

impl OneshotState for ActorRefState {
    type Id = Uuid;

    fn id(&self) -> &Self::Id {
        &self.id
    }

    fn trigger(self) -> () {
        // Ignore the Result, as we are anyway not trying to reschedule this timer
        let _res = self.receiver.enqueue(Timeout(self.id));
    }
}

impl PeriodicState for ActorRefState {
    type Id = Uuid;

    fn id(&self) -> &Self::Id {
        &self.id
    }

    fn trigger(self) -> GenericTimerReturn<Self> {
        // Ignore the Result, as we are anyway not trying to reschedule this timer
        match self.receiver.enqueue(Timeout(self.id)) {
            Ok(_) => GenericTimerReturn::Reschedule(self),
            Err(_) => GenericTimerReturn::Cancel, // Queue has probably been deallocated, so no point in trying this over and over again
        }
    }
}

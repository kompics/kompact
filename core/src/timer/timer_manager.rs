use super::*;
use hierarchical_hash_wheel_timer::{
    Timer as LowlevelTimer,
};

use std::{
    collections::HashMap,
    fmt,
    rc::Rc,
    sync::{Arc, Weak},
    time::Duration,
    thread::{
        current,
    },
};
use uuid::Uuid;

use crate::*;

/// A factory trait to produce instances of [TimerRef](timer::TimerRef)
pub trait TimerRefFactory {
    /// Returns the timer reference for associated with this factory
    fn timer_ref(&self) -> TimerRef;
}

/// Opaque reference to a scheduled instance of a timer
///
/// Use this to cancel the timer with [cancel_timer](Timer::cancel_timer).
///
/// Instances are returned from functions that schedule timers, such as
/// [schedule_once](Timer::schedule_once) and [schedule_periodic](Timer::schedule_periodic).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScheduledTimer(Uuid);

impl ScheduledTimer {
    /// Create a `ScheduledTimer` from a [Uuid](Uuid) handle
    pub fn from_uuid(id: Uuid) -> ScheduledTimer {
        ScheduledTimer(id)
    }
}


/// API exposed within a component by a timer implementation
///
/// This allows behaviours to be scheduled for later execution.
pub trait Timer<C: ComponentDefinition> {
    /// Schedule the `action` to be run once after `timeout` expires
    ///
    /// # Note
    ///
    /// Depending on your system and the implementation used,
    /// there is always a certain lag between the execution of the `action`
    /// and the `timeout` expiring on the system's clock.
    /// Thus it is only guaranteed that the `action` is not run *before*
    /// the `timeout` expires, but no bounds on the lag are given.
    ///
    /// # Example
    ///
    /// ```
    /// use kompact::prelude::*;
    /// use std::time::Duration;
    ///
    /// #[derive(ComponentDefinition, Actor)]
    /// struct TimerComponent {
    ///    ctx: ComponentContext<Self>,
    /// }
    /// impl TimerComponent {
    ///     fn new() -> TimerComponent {
    ///         TimerComponent {
    ///             ctx: ComponentContext::uninitialised(),
    ///         }
    ///     }    
    /// }
    /// impl ComponentLifecycle for TimerComponent {
    ///     fn on_start(&mut self) -> Handled {
    ///         self.schedule_once(Duration::from_millis(10), move |new_self, _id| {
    ///             info!(new_self.log(), "Timeout was triggered!");
    ///             new_self.ctx().system().shutdown_async();
    ///             Handled::Ok
    ///         });
    ///         Handled::Ok
    ///     }    
    /// }
    ///
    /// let system = KompactConfig::default().build().expect("system");
    /// let c = system.create(TimerComponent::new);
    /// system.start(&c);
    /// system.await_termination();
    /// ```
    fn schedule_once<F>(&mut self, timeout: Duration, action: F) -> ScheduledTimer
    where
        F: FnOnce(&mut C, ScheduledTimer) -> Handled + Send + 'static;

    /// Schedule the `action` to be run every `timeout` time units
    ///
    /// The first time, the `action` will be run after `delay` expires,
    /// and then again every `timeout` time units after.
    ///
    /// # Note
    ///
    /// Depending on your system and the implementation used,
    /// there is always a certain lag between the execution of the `action`
    /// and the `timeout` expiring on the system's clock.
    /// Thus it is only guaranteed that the `action` is not run *before*
    /// the `timeout` expires, but no bounds on the lag are given.
    ///
    /// # Example
    ///
    /// ```
    /// use kompact::prelude::*;
    /// use std::time::Duration;
    ///
    /// #[derive(ComponentDefinition, Actor)]
    /// struct TimerComponent {
    ///    ctx: ComponentContext<Self>,
    ///    counter: usize,
    ///    timeout: Option<ScheduledTimer>,
    /// }
    /// impl TimerComponent {
    ///     fn new() -> TimerComponent {
    ///         TimerComponent {
    ///             ctx: ComponentContext::uninitialised(),
    ///             counter: 0usize,
    ///             timeout: None,
    ///         }
    ///     }    
    /// }
    /// impl ComponentLifecycle for TimerComponent {
    ///     fn on_start(&mut self) -> Handled {
    ///         let timeout = self.schedule_periodic(
    ///                 Duration::from_millis(10),
    ///                 Duration::from_millis(100),
    ///                 move |new_self, _id| {
    ///                     info!(new_self.log(), "Timeout was triggered!");
    ///                     new_self.counter += 1usize;
    ///                     if new_self.counter > 10usize {
    ///                        let timeout = new_self.timeout.take().expect("timeout");
    ///                        new_self.cancel_timer(timeout);
    ///                        new_self.ctx().system().shutdown_async();
    ///                     }
    ///                     Handled::Ok
    ///                 }
    ///         );
    ///         self.timeout = Some(timeout);
    ///         Handled::Ok
    ///     }  
    ///     // cleanup timeouts if shut down early, so they don't keep running  
    ///     fn on_stop(&mut self) -> Handled {
    ///         if let Some(timeout) = self.timeout.take() {
    ///             self.cancel_timer(timeout);
    ///         }
    ///         Handled::Ok
    ///     }
    ///     fn on_kill(&mut self) -> Handled {
    ///         if let Some(timeout) = self.timeout.take() {
    ///             self.cancel_timer(timeout);
    ///         }
    ///         Handled::Ok
    ///     }
    /// }
    ///
    /// let system = KompactConfig::default().build().expect("system");
    /// let c = system.create(TimerComponent::new);
    /// system.start(&c);
    /// system.await_termination();
    /// ```
    fn schedule_periodic<F>(
        &mut self,
        delay: Duration,
        period: Duration,
        action: F,
    ) -> ScheduledTimer
    where
        F: Fn(&mut C, ScheduledTimer) -> Handled + Send + 'static;

    /// Cancel the timer indicated by the `handle`
    ///
    /// This method is asynchronous, and calling it is no guarantee
    /// than an already scheduled timeout is not going to fire before it
    /// is actually cancelled.
    ///
    /// However, calling this method will definitely prevent *periodic* timeouts
    /// from being rescheduled.
    fn cancel_timer(&mut self, handle: ScheduledTimer);
}

/// API for cancelling timers
///
/// While only components can schedule timers,
/// cancelling timers can be done anywhere, where a
/// [TimerRef](TimerRef) is available.
pub trait CanCancelTimers {
    /// Cancel the timer indicated by the `handle`
    ///
    /// This method is asynchronous, and calling it is no guarantee
    /// than an already scheduled timeout is not going to fire before it
    /// is actually cancelled.
    ///
    /// However, calling this method will definitely prevent *periodic* timeouts
    /// from being rescheduled.
    fn cancel_timer(&self, handle: ScheduledTimer);
}

impl<F> CanCancelTimers for F
where
    F: TimerRefFactory,
{
    fn cancel_timer(&self, handle: ScheduledTimer) {
        self.timer_ref().cancel(&handle.0);
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Timeout(pub(crate) Uuid);

pub(crate) enum ExecuteAction<C: ComponentDefinition> {
    None,
    Periodic(Uuid, Rc<dyn Fn(&mut C, Uuid) -> Handled>),
    Once(Uuid, Box<dyn FnOnce(&mut C, Uuid) -> Handled>),
}

pub(crate) struct TimerManager<C: ComponentDefinition> {
    timer: TimerRef,
    timer_queue: Arc<ConcurrentQueue<Timeout>>,
    handles: HashMap<Uuid, TimerHandle<C>>,
}

impl<C: ComponentDefinition> TimerManager<C> {
    pub(crate) fn new(timer: TimerRef) -> TimerManager<C> {

        TimerManager {
            timer,
            timer_queue: Arc::new(ConcurrentQueue::new()),
            handles: HashMap::new(),
        }
    }

    fn new_ref(&self, component: Weak<dyn CoreContainer>) -> TimerActorRef {
        TimerActorRef::new(component, Arc::downgrade(&self.timer_queue))
    }

    pub(crate) fn try_action(&mut self) -> ExecuteAction<C> {
        if let Ok(timeout) = self.timer_queue.pop() {
            let res = self.handles.remove(&timeout.0);
            match res {
                Some(TimerHandle::OneShot { action, .. }) => ExecuteAction::Once(timeout.0, action),
                Some(TimerHandle::Periodic { action, .. }) => {
                    let action2 = action.clone();
                    self.handles.insert(
                        timeout.0,
                        TimerHandle::Periodic {
                            _id: timeout.0,
                            action,
                        },
                    );
                    ExecuteAction::Periodic(timeout.0, action2)
                }
                _ => ExecuteAction::None,
            }
        } else {
            ExecuteAction::None
        }
    }

    pub(crate) fn schedule_once<F>(
        &mut self,
        component: Weak<dyn CoreContainer>,
        timeout: Duration,
        action: F,
    ) -> ScheduledTimer
    where
        F: FnOnce(&mut C, ScheduledTimer) -> Handled + Send + 'static,
    {
        let id = Uuid::new_v4();
        let handle = TimerHandle::OneShot {
            _id: id,
            action: Box::new(move |new_self, id| action(new_self, ScheduledTimer::from_uuid(id))),
        };
        self.handles.insert(id, handle);
        let tar = self.new_ref(component);
        let state = ActorRefState::new(id, tar);
        self.timer.schedule_once(timeout, state);
        ScheduledTimer::from_uuid(id)
    }

    pub(crate) fn schedule_periodic<F>(
        &mut self,
        component: Weak<dyn CoreContainer>,
        delay: Duration,
        period: Duration,
        action: F,
    ) -> ScheduledTimer
    where
        F: Fn(&mut C, ScheduledTimer) -> Handled + Send + 'static,
    {
        let id = Uuid::new_v4();
        let handle = TimerHandle::Periodic {
            _id: id,
            action: Rc::new(move |new_self, id| action(new_self, ScheduledTimer::from_uuid(id))),
        };
        self.handles.insert(id, handle);
        let tar = self.new_ref(component);
        let state = ActorRefState::new(id, tar);
        self.timer.schedule_periodic(delay, period, state);
        ScheduledTimer::from_uuid(id)
    }

    pub(crate) fn cancel_timer(&mut self, handle: ScheduledTimer) {
        self.timer.cancel(&handle.0);
        self.handles.remove(&handle.0);
    }
}

impl<C: ComponentDefinition> Drop for TimerManager<C> {
    fn drop(&mut self) {
        // drop all handles in case someone forgets to clean up after their component
        for (id, _) in self.handles.drain() {
            self.timer.cancel(&id);
        }
    }
}

// NEVER SEND THIS!
pub(crate) enum TimerHandle<C: ComponentDefinition> {
    OneShot {
        _id: Uuid, // not used atm
        action: Box<dyn FnOnce(&mut C, Uuid) -> Handled + Send + 'static>,
    },
    Periodic {
        _id: Uuid, // not used atm
        action: Rc<dyn Fn(&mut C, Uuid) -> Handled + Send + 'static>,
    },
}

// This isn't technically true, but I know I'm never actually sending
// individual Rc instances to different threads. Only the whole component
// with all its Rc instances crosses threads sometimes.
#[allow(clippy::non_send_fields_in_send_ty)]
unsafe impl<C: ComponentDefinition> Send for TimerHandle<C> {}

#[derive(Clone)]
pub(crate) struct TimerActorRef {
    component: Weak<dyn CoreContainer>,
    msg_queue: Weak<ConcurrentQueue<Timeout>>,
}

impl TimerActorRef {
    fn new(
        component: Weak<dyn CoreContainer>,
        msg_queue: Weak<ConcurrentQueue<Timeout>>,
    ) -> TimerActorRef {
        TimerActorRef {
            component,
            msg_queue,
        }
    }

    pub(crate) fn enqueue(&self, timeout: Timeout) -> Result<(), QueueingError> {
        match (self.msg_queue.upgrade(), self.component.upgrade()) {
            (Some(q), Some(c)) => {
                let res = c.core().increment_work();
                q.push(timeout);
                println!("TIMER");
                if let SchedulingDecision::Schedule = res {
                    let system = c.core().system();
                    system.schedule(c.clone());
                }
                Ok(())
            }
            (q, c) => {
                eprintln!("Dropping timeout as target (queue? {:?}, component? {:?}) is unavailable: {:?}",
                    q.is_some(),
                    c.is_some(),
                    timeout
                );
                Err(QueueingError)
            }
        }
    }
}

impl fmt::Debug for TimerActorRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "<timer-actor-ref>")
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct QueueingError;

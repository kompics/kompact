use crate::timer::Timer as TTimer;
use std::{
    collections::HashMap,
    rc::Rc,
    sync::{Arc, Weak},
    time::Duration,
};
use uuid::Uuid;

use super::*;

/// A factory trait to produce instances of `TimerRef`(timer::TimerRef)
pub trait TimerRefFactory {
    fn timer_ref(&self) -> timer::TimerRef;
}

/// Opaque reference to a scheduled instance of a timer
///
/// Use this to cancel the timer with [cancel_timer](Timer::cancel_timer).
///
/// Instances are returned from functions that schedule timers, such as
/// [schedule_once](Timer::schedule_once) and [schedule_periodic](Timer::schedule_periodic).
#[derive(Clone, Debug)]
pub struct ScheduledTimer(Uuid);

impl ScheduledTimer {
    fn from_uuid(id: Uuid) -> ScheduledTimer {
        ScheduledTimer(id)
    }
}

/// API exposed by a timer implementation
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
    fn schedule_once<F>(&mut self, timeout: Duration, action: F) -> ScheduledTimer
    where
        F: FnOnce(&mut C, Uuid) + Send + 'static;

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
    fn schedule_periodic<F>(
        &mut self,
        delay: Duration,
        period: Duration,
        action: F,
    ) -> ScheduledTimer
    where
        F: Fn(&mut C, Uuid) + Send + 'static;

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

#[derive(Clone, Debug)]
pub(crate) struct Timeout(Uuid);

pub(crate) enum ExecuteAction<C: ComponentDefinition> {
    None,
    Periodic(Uuid, Rc<dyn Fn(&mut C, Uuid)>),
    Once(Uuid, Box<dyn FnOnce(&mut C, Uuid)>),
}

pub(crate) struct TimerManager<C: ComponentDefinition> {
    timer: timer::TimerRef,
    timer_queue: Arc<ConcurrentQueue<Timeout>>,
    handles: HashMap<Uuid, TimerHandle<C>>,
}

impl<C: ComponentDefinition> TimerManager<C> {
    pub(crate) fn new(timer: timer::TimerRef) -> TimerManager<C> {
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
        F: FnOnce(&mut C, Uuid) + Send + 'static,
    {
        let id = Uuid::new_v4();
        let handle = TimerHandle::OneShot {
            _id: id,
            action: Box::new(action),
        };
        self.handles.insert(id, handle);
        let tar = self.new_ref(component);
        self.timer.schedule_once(id, timeout, move |id| {
            tar.enqueue(Timeout(id));
        });
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
        F: Fn(&mut C, Uuid) + Send + 'static,
    {
        let id = Uuid::new_v4();
        let handle = TimerHandle::Periodic {
            _id: id,
            action: Rc::new(action),
        };
        self.handles.insert(id, handle);
        let tar = self.new_ref(component);
        self.timer.schedule_periodic(id, delay, period, move |id| {
            tar.enqueue(Timeout(id));
        });
        ScheduledTimer::from_uuid(id)
    }

    pub(crate) fn cancel_timer(&mut self, handle: ScheduledTimer) {
        self.timer.cancel(handle.0);
        self.handles.remove(&handle.0);
    }
}

// NEVER SEND THIS!
pub(crate) enum TimerHandle<C: ComponentDefinition> {
    OneShot {
        _id: Uuid, // not used atm
        action: Box<dyn FnOnce(&mut C, Uuid) + Send + 'static>,
    },
    Periodic {
        _id: Uuid, // not used atm
        action: Rc<dyn Fn(&mut C, Uuid) + Send + 'static>,
    },
}

// This isn't technically true, but I know I'm never actually sending 
// individual Rc instances to different threads. Only the whole component
// with all its Rc instances crosses threads sometimes.
unsafe impl<C: ComponentDefinition> Send for TimerHandle<C> {} 

#[derive(Clone)]
struct TimerActorRef {
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

    fn enqueue(&self, timeout: Timeout) -> () {
        match (self.msg_queue.upgrade(), self.component.upgrade()) {
            (Some(q), Some(c)) => {
                q.push(timeout);
                match c.core().increment_work() {
                    SchedulingDecision::Schedule => {
                        let system = c.core().system();
                        system.schedule(c.clone());
                    }
                    _ => (), // nothing
                }
            }
            (q, c) => println!(
                "Dropping timeout as target (queue? {:?}, component? {:?}) is unavailable: {:?}",
                q.is_some(),
                c.is_some(),
                timeout
            ),
        }
    }
}

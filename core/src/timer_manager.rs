use crossbeam::sync::MsQueue;
use std::boxed::FnBox;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Weak};
use std::time::Duration;
use crate::timer::Timer as TTimer;
use uuid::Uuid;

use super::*;

pub trait TimerRefFactory {
    fn timer_ref(&self) -> timer::TimerRef;
}

#[derive(Clone, Debug)]
pub struct ScheduledTimer(Uuid);

impl ScheduledTimer {
    fn from_uuid(id: Uuid) -> ScheduledTimer {
        ScheduledTimer(id)
    }
}

pub trait Timer<C: ComponentDefinition> {
    fn schedule_once<F>(&mut self, timeout: Duration, action: F) -> ScheduledTimer
    where
        F: FnOnce(&mut C, Uuid) + Send + 'static;

    fn schedule_periodic<F>(
        &mut self,
        delay: Duration,
        period: Duration,
        action: F,
    ) -> ScheduledTimer
    where
        F: Fn(&mut C, Uuid) + Send + 'static;

    fn cancel_timer(&mut self, handle: ScheduledTimer);
}

#[derive(Clone, Debug)]
pub(crate) struct Timeout(Uuid);

pub(crate) enum ExecuteAction<C: ComponentDefinition> {
    None,
    Periodic(Uuid, Rc<Fn(&mut C, Uuid)>),
    Once(Uuid, Box<FnBox(&mut C, Uuid)>),
}

pub(crate) struct TimerManager<C: ComponentDefinition> {
    timer: timer::TimerRef,
    timer_queue: Arc<MsQueue<Timeout>>,
    handles: HashMap<Uuid, TimerHandle<C>>,
}

impl<C: ComponentDefinition> TimerManager<C> {
    pub(crate) fn new(timer: timer::TimerRef) -> TimerManager<C> {
        TimerManager {
            timer,
            timer_queue: Arc::new(MsQueue::new()),
            handles: HashMap::new(),
        }
    }

    fn new_ref(&self, component: Weak<CoreContainer>) -> TimerActorRef {
        TimerActorRef::new(component, Arc::downgrade(&self.timer_queue))
    }

    pub(crate) fn try_action(&mut self) -> ExecuteAction<C> {
        if let Some(timeout) = self.timer_queue.try_pop() {
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
        component: Weak<CoreContainer>,
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
        component: Weak<CoreContainer>,
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

pub(crate) enum TimerHandle<C: ComponentDefinition> {
    OneShot {
        _id: Uuid, // not used atm
        action: Box<FnBox(&mut C, Uuid) + Send + 'static>,
    },
    Periodic {
        _id: Uuid, // not used atm
        action: Rc<Fn(&mut C, Uuid) + Send + 'static>,
    },
}

unsafe impl<C: ComponentDefinition> Send for TimerHandle<C> {} // this isn't technically true, but I know I'm never actually sending it

#[derive(Clone)]
struct TimerActorRef {
    component: Weak<CoreContainer>,
    msg_queue: Weak<MsQueue<Timeout>>,
}

impl TimerActorRef {
    fn new(component: Weak<CoreContainer>, msg_queue: Weak<MsQueue<Timeout>>) -> TimerActorRef {
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

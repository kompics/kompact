use std::boxed::FnBox;
use std::fmt;
use std::io;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant, SystemTime};
use uuid::Uuid;

pub use self::wheels::*;
pub use self::simulation::*;
pub use self::thread_timer::*;

mod wheels;
mod simulation;
mod thread_timer;

pub trait Timer {
    fn schedule_once<F>(&mut self, id: Uuid, timeout: Duration, action: F) -> ()
    where
        F: FnOnce(Uuid) + Send + 'static;
    fn schedule_periodic<F>(
        &mut self,
        id: Uuid,
        delay: Duration,
        period: Duration,
        action: F,
    ) -> ()
    where
        F: Fn(Uuid) + Send + 'static;
    fn cancel(&mut self, id: Uuid);
}

pub enum TimerEntry {
    OneShot {
        id: Uuid,
        timeout: Duration,
        action: Box<FnBox(Uuid) + Send + 'static>,
    },
    Periodic {
        id: Uuid,
        delay: Duration,
        period: Duration,
        action: Box<Fn(Uuid) + Send + 'static>,
    },
}

impl TimerEntry {
    pub fn id(&self) -> Uuid {
        match self {
            TimerEntry::OneShot { id, .. } => *id,
            TimerEntry::Periodic { id, .. } => *id,
        }
    }

    pub fn id_ref(&self) -> &Uuid {
        match self {
            TimerEntry::OneShot { id, .. } => id,
            TimerEntry::Periodic { id, .. } => id,
        }
    }

    pub fn delay(&self) -> Duration {
        match self {
            TimerEntry::OneShot { timeout, .. } => *timeout,
            TimerEntry::Periodic { delay, .. } => *delay,
        }
    }

    pub fn delay_ref(&self) -> &Duration {
        match self {
            TimerEntry::OneShot { timeout, .. } => timeout,
            TimerEntry::Periodic { delay, .. } => delay,
        }
    }

    pub fn with_duration(self, d: Duration) -> TimerEntry {
        match self {
            TimerEntry::OneShot { id, action, .. } => TimerEntry::OneShot {
                id,
                timeout: d,
                action,
            },
            TimerEntry::Periodic {
                id, period, action, ..
            } => TimerEntry::Periodic {
                id,
                delay: d,
                period,
                action,
            },
        }
    }

    pub fn execute(self) -> Option<TimerEntry> {
        match self {
            TimerEntry::OneShot { id, action, .. } => {
                //FnOnceBox::call_box(action, id);
                action.call_box((id,));
                None
            }
            TimerEntry::Periodic {
                id, action, period, ..
            } => {
                action.as_ref()(id);
                let next = TimerEntry::Periodic {
                    id,
                    delay: period,
                    period,
                    action,
                };
                //FnBox::call_box(&action, id);
                Some(next)
            }
        }
    }
}

impl fmt::Debug for TimerEntry {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            TimerEntry::OneShot { id, timeout, .. } => write!(
                f,
                "TimerEntry::OneShot(id={:?}, timeout={:?}, action=<function>)",
                id, timeout
            ),
            TimerEntry::Periodic {
                id, delay, period, ..
            } => write!(
                f,
                "TimerEntry::Periodic(id={:?}, delay={:?}, period={:?} action=<function>)",
                id, delay, period
            ),
        }
    }
}

type TimerList = Vec<TimerEntry>;

#[derive(Debug)]
pub enum TimerError {
    NotFound,
    Expired(TimerEntry),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn simple_simulation() {
        let num = 10usize;
        let mut barriers: Vec<Arc<Mutex<bool>>> = Vec::with_capacity(num);
        let mut timer = SimulationTimer::new();
        for i in 0..num {
            let barrier = Arc::new(Mutex::new(false));
            barriers.push(barrier.clone());
            let id = Uuid::new_v4();
            let timeout = Duration::from_millis(150u64 * (i as u64));
            timer.schedule_once(id, timeout, move |_| {
                println!("Running action {}", i);
                let mut guard = barrier.lock().unwrap();
                *guard = true;
            });
        }
        let mut running = true;
        while running {
            match timer.next() {
                SimulationStep::Ok => println!("Next!"),
                SimulationStep::Finished => running = false,
            }
        }
        println!("Simulation run done!");
        for b in barriers {
            let guard = b.lock().unwrap();
            assert_eq!(*guard, true);
        }
    }

    // TODO test with rescheduling

    #[test]
    fn simple_thread_timing() {
        let num = 10usize;
        let mut barriers: Vec<Arc<Mutex<bool>>> = Vec::with_capacity(num);
        let mut timer_core = TimerWithThread::new().expect("Timer thread didn't load properly!");
        let mut timer = timer_core.timer_ref();
        let mut total_wait = 0u64;
        println!("Starting timing run.");
        for i in 0..num {
            let barrier = Arc::new(Mutex::new(false));
            barriers.push(barrier.clone());
            let id = Uuid::new_v4();
            let time = 150u64 * (i as u64);
            total_wait += time;
            let timeout = Duration::from_millis(time);
            let now = Instant::now();
            timer.schedule_once(id, timeout, move |_| {
                let elap = now.elapsed().as_nanos();
                let target = timeout.as_nanos();
                if (elap > target) {
                    let diff = ((elap - target) as f64)/1000000.0;
                    println!("Running action {} {}ms late", i, diff);
                } else {
                    let diff = ((target - elap) as f64)/1000000.0;
                    println!("Running action {} {}ms early", i, diff);
                }
                let mut guard = barrier.lock().unwrap();
                *guard = true;
            });
        }
        println!("Waiting timing run to finish {}ms", total_wait);
        thread::sleep(Duration::from_millis(total_wait));
        timer_core.shutdown().expect("Timer didn't shutdown properly!");
        println!("Timing run done!");
        for b in barriers {
            let guard = b.lock().unwrap();
            assert_eq!(*guard, true);
        }
    }
}

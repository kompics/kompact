use super::*;

/// A timer implementation that used virtual time
///
/// Time is simply advanced until the next event is scheduled.
pub struct SimulationTimer {
    time: u128,
    timer: QuadWheelWithOverflow,
}

impl SimulationTimer {
    /// Create a new simulation timer starting at `0`
    pub fn new() -> SimulationTimer {
        SimulationTimer {
            time: 0u128,
            timer: QuadWheelWithOverflow::new(),
        }
    }

    /// Create a new simulation timer starting at a system clock value
    pub fn at(now: SystemTime) -> SimulationTimer {
        let t = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("SystemTime before UNIX EPOCH!");
        let tms = t.as_millis();
        SimulationTimer {
            time: tms,
            timer: QuadWheelWithOverflow::new(),
        }
    }

    /// Return the timers current virtual time value (in ms)
    pub fn current_time(&self) -> u128 {
        self.time
    }

    /// Advance the virtual time
    pub fn next(&mut self) -> SimulationStep {
        loop {
            match self.timer.can_skip() {
                Skip::Empty => return SimulationStep::Finished,
                Skip::None => {
                    let mut res = self.timer.tick();
                    self.time += 1u128;
                    if !res.is_empty() {
                        for e in res.drain(..) {
                            e.execute();
                        }
                        return SimulationStep::Ok;
                    }
                }
                Skip::Millis(ms) => {
                    self.timer.skip(ms);
                    self.time += ms as u128;
                    let mut res = self.timer.tick();
                    self.time += 1u128;
                    if !res.is_empty() {
                        for e in res.drain(..) {
                            e.execute();
                        }
                        return SimulationStep::Ok;
                    }
                }
            }
        }
    }
}

/// Result of advancing virtual time
pub enum SimulationStep {
    /// No timeout remain
    ///
    /// The simulation can be considered complete.
    Finished,
    /// Step was executed, but timeouts remain
    ///
    /// Continue calling [next](SimulationTimer::next) to advance virtual time.
    Ok,
}

impl Timer for SimulationTimer {
    fn schedule_once<F>(&mut self, id: Uuid, timeout: Duration, action: F) -> ()
    where
        F: FnOnce(Uuid) + Send + 'static,
    {
        let e = TimerEntry::OneShot {
            id,
            timeout,
            action: Box::new(action),
        };
        match self.timer.insert(e) {
            Ok(_) => (), // ok
            Err(TimerError::Expired(e)) => {
                if let None = e.execute() {
                    ()
                } else {
                    panic!("OneShot produced reschedule!")
                } // clearly a OneShot
            }
            Err(f) => panic!("Could not insert timer entry! {:?}", f),
        }
    }

    fn schedule_periodic<F>(&mut self, id: Uuid, delay: Duration, period: Duration, action: F) -> ()
    where
        F: Fn(Uuid) -> TimerReturn + Send + 'static,
    {
        let e = TimerEntry::Periodic {
            id,
            delay,
            period,
            action: Box::new(action),
        };
        match self.timer.insert(e) {
            Ok(_) => (), // ok
            Err(TimerError::Expired(e)) => match e.execute() {
                Some(new_e) => match self.timer.insert(new_e) {
                    Ok(_) => (), // ok
                    Err(TimerError::Expired(e)) => panic!(
                        "Trying to insert periodic timer entry with 0ms period! {:?}",
                        e
                    ),
                    Err(f) => panic!("Could not insert timer entry! {:?}", f),
                },
                None => unreachable!(), // since it clearly is a periodic timer, it better be rescheduled
            },
            Err(f) => panic!("Could not insert timer entry! {:?}", f),
        }
    }

    fn cancel(&mut self, id: Uuid) {
        match self.timer.cancel(id) {
            Ok(_) => (),                                                           // great
            Err(f) => eprintln!("Could not cancel timer with id={}. {:?}", id, f), // not so great, but meh
        }
    }
}

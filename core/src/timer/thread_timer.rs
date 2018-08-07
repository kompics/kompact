use super::*;

use crossbeam_channel as channel;

#[derive(Debug)]
enum TimerMsg {
    Schedule(TimerEntry),
    Cancel(Uuid),
    Stop,
}

#[derive(Clone)]
pub struct TimerRef {
    work_queue: channel::Sender<TimerMsg>,
}

impl Timer for TimerRef {
    fn schedule_once<F>(&mut self, id: Uuid, timeout: Duration, action: F) -> ()
    where
        F: FnOnce(Uuid) + Send + 'static,
    {
        let e = TimerEntry::OneShot {
            id,
            timeout,
            action: Box::new(action),
        };
        self.work_queue.send(TimerMsg::Schedule(e));
    }

    fn schedule_periodic<F>(&mut self, id: Uuid, delay: Duration, period: Duration, action: F) -> ()
    where
        F: Fn(Uuid) + Send + 'static,
    {
        let e = TimerEntry::Periodic {
            id,
            delay,
            period,
            action: Box::new(action),
        };
        self.work_queue.send(TimerMsg::Schedule(e));
    }
    fn cancel(&mut self, id: Uuid) {
        self.work_queue.send(TimerMsg::Cancel(id));
    }
}

pub struct TimerWithThread {
    timer_thread: thread::JoinHandle<()>,
    work_queue: channel::Sender<TimerMsg>,
}

impl TimerWithThread {
    pub fn new() -> io::Result<TimerWithThread> {
        let (s, r) = channel::unbounded();
        let handle = thread::Builder::new()
            .name("timer-thread".to_string())
            .spawn(move || {
                let timer = TimerThread::new(r);
                timer.run();
            })?;
        let twt = TimerWithThread {
            timer_thread: handle,
            work_queue: s,
        };
        Ok(twt)
    }

    pub fn timer_ref(&self) -> TimerRef {
        TimerRef {
            work_queue: self.work_queue.clone(),
        }
    }

    pub fn shutdown(self) -> Result<(), ThreadTimerError> {
        self.work_queue.send(TimerMsg::Stop);
        match self.timer_thread.join() {
            Ok(_) => Ok(()),
            Err(_) => {
                eprintln!("Timer thread panicked!");
                Err(ThreadTimerError::CouldNotJoinThread)
            }
        }
    }

    pub fn shutdown_async(&self) -> Result<(), ThreadTimerError> {
        self.work_queue.send(TimerMsg::Stop);
        Ok(())
        //.map_err(|_| ThreadTimerError::CouldNotSendStopAsync)
    }
}

impl fmt::Debug for TimerWithThread {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "<TimerWithThread>")
    }
}

#[derive(Debug)]
pub enum ThreadTimerError {
    CouldNotSendStopAsync,
    CouldNotSendStop(TimerWithThread),
    CouldNotJoinThread,
}

pub struct TimerThread {
    timer: QuadWheelWithOverflow,
    work_queue: channel::Receiver<TimerMsg>,
    running: bool,
    start: Instant,
    last_check: u128,
}

impl TimerThread {
    fn new(work_queue: channel::Receiver<TimerMsg>) -> TimerThread {
        TimerThread {
            timer: QuadWheelWithOverflow::new(),
            work_queue,
            running: true,
            start: Instant::now(),
            last_check: 0u128,
        }
    }

    pub fn run(mut self) {
        while self.running {
            let elap = self.elapsed();
            if elap > 0 {
                for _ in 0..elap {
                    self.tick();
                }
            } else {
                match self.work_queue.try_recv() {
                    Some(msg) => self.handle_msg(msg),
                    None => {
                        // nothing to tick and nothing in the queue
                        match self.timer.can_skip() {
                            Skip::None => {
                                thread::yield_now();
                                // if last_check.elapsed().as_nanos() > 800000 {
                                //     // if more than 8/10ms left
                                //     thread::yield_now(); // just let someone else run until the next tick
                                // } // else hot wait to reduce inaccuracy
                            }
                            Skip::Empty => {
                                // wait until something is scheduled
                                // don't even need to bother skipping time in the wheel,
                                // since all times in there are relative
                                match self.work_queue.recv() {
                                    Some(msg) => {
                                        self.reset(); // since we waited for an arbitrary time and taking a new timestamp incures no error
                                        self.handle_msg(msg)
                                    }
                                    None => panic!("Timer work_queue unexpectedly shut down!"),
                                }
                            }
                            Skip::Millis(ms) if ms > 5 => {
                                let ms = ms - 5; // balance OS scheduler inaccuracy
                                                 // wait until something is scheduled but max skip
                                let timeout = Duration::from_millis(ms as u64);
                                let res = select! {
                                    recv(self.work_queue, msg) => msg,
                                    recv(channel::after(timeout)) => None,
                                };
                                let elap = self.elapsed();
                                let longms = ms as u128;
                                if elap > longms {
                                    // took longer to get rescheduled than we wanted
                                    self.timer.skip(ms);
                                    for _ in 0..(elap - longms) {
                                        self.tick();
                                    }
                                } else if elap < longms {
                                    // we got woken up early, no need to tick
                                    self.timer.skip(elap as u32)
                                } else {
                                    // elap == ms
                                    // next action should be a tick, but add the new items first
                                    self.timer.skip(ms);
                                }
                                match res {
                                    Some(msg) => self.handle_msg(msg),
                                    None => (), // restart loop
                                }
                            }
                            Skip::Millis(_) => {
                                thread::yield_now();
                                // if last_check.elapsed().as_nanos() > 800000 {
                                //     // if more than 8/10ms left
                                //     thread::yield_now(); // just let someone else run until the next tick
                                // } // else hot wait to reduce inaccuracy
                            }
                        }
                    }
                }
            }
        }
    }

    #[inline(always)]
    fn elapsed(&mut self) -> u128 {
        let elap = self.start.elapsed().as_millis();
        let rel_elap = elap - self.last_check;
        self.last_check = elap;
        rel_elap
    }

    #[inline(always)]
    fn reset(&mut self) {
        self.start = Instant::now();
        self.last_check = 0;
    }

    #[inline(always)]
    fn handle_msg(&mut self, msg: TimerMsg) -> () {
        match msg {
            TimerMsg::Stop => self.running = false,
            TimerMsg::Schedule(e) => {
                match self.timer.insert(e) {
                    Ok(_) => (), // ok
                    Err(TimerError::Expired(e)) => {
                        self.execute(e);
                    }
                    Err(f) => panic!("Could not insert timer entry! {:?}", f),
                }
            }
            TimerMsg::Cancel(id) => match self.timer.cancel(id) {
                Ok(_) => (),                     // ok
                Err(TimerError::NotFound) => (), // also ok, might have been triggered already
                Err(f) => panic!("Unexpected error cancelling timer! {:?}", f),
            },
        }
    }

    #[inline(always)]
    fn execute(&mut self, e: TimerEntry) -> () {
        match e.execute() {
            Some(re_e) => match self.timer.insert(re_e) {
                Ok(_) => (), // great
                Err(TimerError::Expired(re_e)) => {
                    // This could happen if someone specifies 0ms period
                    eprintln!("TimerEntry could not be inserted properly: {:?}", re_e);
                }
                Err(f) => panic!("Could not insert timer entry! {:?}", f),
            },
            None => (), // great
        }
    }

    #[inline(always)]
    fn tick(&mut self) -> () {
        let mut res = self.timer.tick();
        for e in res.drain(..) {
            self.execute(e);
        }
    }
}

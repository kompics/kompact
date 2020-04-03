use super::*;

use channel::select;
use crossbeam_channel as channel;

#[derive(Debug)]
enum TimerMsg {
    Schedule(TimerEntry),
    Cancel(Uuid),
    Stop,
}

/// A reference to a thread timer
///
/// This is used to schedule events on the timer from other threads.
///
/// You can get an instance via [timer_ref](TimerWithThread::timer_ref).
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
        self.work_queue
            .send(TimerMsg::Schedule(e))
            .unwrap_or_else(|e| eprintln!("Could not send Schedule msg: {:?}", e));
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
        self.work_queue
            .send(TimerMsg::Schedule(e))
            .unwrap_or_else(|e| eprintln!("Could not send Schedule msg: {:?}", e));
    }

    fn cancel(&mut self, id: Uuid) {
        self.work_queue
            .send(TimerMsg::Cancel(id))
            .unwrap_or_else(|e| eprintln!("Could not send Cancel msg: {:?}", e));
    }
}

/// A timer implementation that uses its own thread
///
/// This struct acts as a main handle for the timer and its thread.
pub struct TimerWithThread {
    timer_thread: thread::JoinHandle<()>,
    work_queue: channel::Sender<TimerMsg>,
}

impl TimerWithThread {
    /// Create a new timer with its own thread.
    ///
    /// The thread will be called `"timer-thread"`.
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

    /// Returns a shareable reference to this timer
    ///
    /// The reference contains the timer's work queue
    /// and can be used to schedule timeouts on this timer.
    pub fn timer_ref(&self) -> TimerRef {
        TimerRef {
            work_queue: self.work_queue.clone(),
        }
    }

    /// Shut this timer down
    ///
    /// In particular, this method waits for the timer's thread to be
    /// joined, or returns an error.
    pub fn shutdown(self) -> Result<(), ThreadTimerError> {
        self.work_queue
            .send(TimerMsg::Stop)
            .unwrap_or_else(|e| eprintln!("Could not send Stop msg: {:?}", e));
        match self.timer_thread.join() {
            Ok(_) => Ok(()),
            Err(_) => {
                eprintln!("Timer thread panicked!");
                Err(ThreadTimerError::CouldNotJoinThread)
            }
        }
    }

    /// Same as [shutdown](TimerWithThread::shutdown), but doesn't wait for the thread to join
    pub fn shutdown_async(&self) -> Result<(), ThreadTimerError> {
        self.work_queue
            .send(TimerMsg::Stop)
            .unwrap_or_else(|e| eprintln!("Could not send Stop msg: {:?}", e));
        Ok(())
    }
}

impl fmt::Debug for TimerWithThread {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<TimerWithThread>")
    }
}

/// Errors that can occur when stopping the timer thread
#[derive(Debug)]
pub enum ThreadTimerError {
    /// Sending of the `Stop` message failed
    CouldNotSendStopAsync,
    /// Sending of the `Stop` message failed in the waiting case
    ///
    /// This variant returns the original timer instance.
    CouldNotSendStop(TimerWithThread),
    /// Joining of the timer thread failed
    CouldNotJoinThread,
}

struct TimerThread {
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

    fn run(mut self) {
        while self.running {
            let elap = self.elapsed();
            if elap > 0 {
                for _ in 0..elap {
                    self.tick();
                }
            }

            match self.work_queue.try_recv() {
                Ok(msg) => self.handle_msg(msg),
                Err(channel::TryRecvError::Empty) => {
                    match self.timer.can_skip() {
                        Skip::None => {
                            () // hot wait
                        }
                        Skip::Empty => {
                            // wait until something is scheduled
                            // don't even need to bother skipping time in the wheel,
                            // since all times in there are relative
                            match self.work_queue.recv() {
                                Ok(msg) => {
                                    self.reset(); // since we waited for an arbitrary time and taking a new timestamp incurs no error
                                    self.handle_msg(msg)
                                }
                                Err(channel::RecvError) => {
                                    panic!("Timer work_queue unexpectedly shut down!")
                                }
                            }
                        }
                        Skip::Millis(can_skip) if can_skip > 5 => {
                            let waiting_time = can_skip - 5; // balance OS scheduler inaccuracy
                                                             // wait until something is scheduled but max skip
                            let timeout = Duration::from_millis(waiting_time as u64);
                            let res = select! {
                                recv(self.work_queue) -> msg => msg.ok(),
                                default(timeout) => None,
                            };
                            let elap = self.elapsed();
                            self.skip_and_tick(can_skip, elap);
                            match res {
                                Some(msg) => self.handle_msg(msg),
                                None => (), // restart loop
                            }
                        }
                        Skip::Millis(can_skip) => {
                            thread::yield_now();
                            let elap = self.elapsed();
                            self.skip_and_tick(can_skip, elap);
                        }
                    }
                }
                Err(channel::TryRecvError::Disconnected) => {
                    panic!("Timer work_queue unexpectedly shut down!")
                }
            }
        }
    }

    #[inline(always)]
    fn skip_and_tick(&mut self, can_skip: u32, elapsed: u128) -> () {
        let can_skip_u128 = can_skip as u128;
        if elapsed > can_skip_u128 {
            // took longer to get rescheduled than we wanted
            self.timer.skip(can_skip);
            let ticks = elapsed - can_skip_u128;
            for _ in 0..ticks {
                self.tick();
            }
        } else if elapsed < can_skip_u128 {
            // we got woken up early, no need to tick
            self.timer.skip(elapsed as u32);
        } else {
            // elapsed == can_skip
            self.timer.skip(can_skip);
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

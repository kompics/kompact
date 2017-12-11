use super::*;

use crossbeam_channel::*;
use uuid::*;
use std::sync::Mutex;
use std::time::Duration;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

pub struct CrossbeamExperiment;

struct CrossbeamExperimentInstance {
    scheduler: CrossbeamPool,
    events: u64,
    latch: Arc<CountdownEvent>,
    in_parallelism: usize,
}

impl Experiment for CrossbeamExperiment {
    fn setup(events: u64, in_parallelism: usize, out_threads: usize) -> Box<ExperimentInstance> {
        let inst = CrossbeamExperimentInstance {
            scheduler: CrossbeamPool::new(out_threads),
            events,
            latch: Arc::new(CountdownEvent::new(in_parallelism)),
            in_parallelism,
        };
        Box::new(inst) as Box<ExperimentInstance>
    }
    fn label() -> String {
        String::from("CrossbeamPool")
    }
}

impl ExperimentInstance for CrossbeamExperimentInstance {
    fn run(&mut self) {
        for i in 0..self.in_parallelism {
            let sched = self.scheduler.clone();
            let events = self.events - 1;
            let latch = self.latch.clone();
            thread::spawn(move || {
                let finish_latch = Arc::new(CountdownEvent::new(1));
                let finisher = Finisher::new(finish_latch.clone());
                for j in 0..events {
                    sched.execute(move || { ignore((i as u64) + j); });
                }
                sched.execute(move || finisher.execute());
                finish_latch.wait();
                ignore(latch.decrement());
            });
        }
        self.latch.wait();
    }
    fn label(&self) -> String {
        CrossbeamExperiment::label()
    }
}

struct Finisher {
    latch: Arc<CountdownEvent>,
}

impl Finisher {
    fn new(latch: Arc<CountdownEvent>) -> Finisher {
        Finisher { latch }
    }
    fn execute(self) {
        ignore(self.latch.decrement());
    }
}

pub trait CrossbeamPoolHandle {
    fn execute<F>(&self, job: F)
    where
        F: FnOnce() + Send + 'static;
    fn shutdown_async(&self);
    fn shutdown(&self) -> Result<(), String>;
}

#[derive(Clone)]
pub struct CrossbeamPool {
    inner: Arc<Mutex<CrossbeamPoolCore>>,
    sender: Sender<JobMsg>,
    threads: usize,
    shutdown: Arc<AtomicBool>,
}

impl CrossbeamPool {
    fn new(threads: usize) -> CrossbeamPool {
        let (tx, rx) = unbounded();
        let shutdown = Arc::new(AtomicBool::new(false));
        let pool = CrossbeamPool {
            inner: Arc::new(Mutex::new(CrossbeamPoolCore {
                sender: tx.clone(),
                receiver: rx.clone(),
                shutdown: shutdown.clone(),
                threads,
            })),
            sender: tx.clone(),
            threads,
            shutdown,
        };
        for i in 0..threads {
            let mut worker = CrossbeamWorker::new(i, rx.clone());
            thread::spawn(move || worker.run());
        }
        pool
    }
}

impl CrossbeamPoolHandle for CrossbeamPool {
    fn execute<F>(&self, job: F)
    where
        F: FnOnce() + Send + 'static,
    {
        // NOTE; This check costs about 150k schedulings/s in a 2 by 2 experiment over 20 runs.
        if !self.shutdown.load(Ordering::SeqCst) { 
            self.sender.send(JobMsg::Job(Box::new(job))).expect(
                "Couldn't schedule job in queue!",
            );
        } else {
            println!("Ignoring job as pool is shutting down.")
        }
    }
    fn shutdown_async(&self) {
        if !self.shutdown.compare_and_swap(
            false,
            true,
            Ordering::SeqCst,
        )
        {
            let latch = Arc::new(CountdownEvent::new(self.threads));
            println!("Shutting down {} threads", self.threads);
            for _ in 0..self.threads {
                self.sender.send(JobMsg::Stop(latch.clone()));
            }
        } else {
            println!("Pool is already shutting down!");
        }
    }
    fn shutdown(&self) -> Result<(), String> {
        if !self.shutdown.compare_and_swap(
            false,
            true,
            Ordering::SeqCst,
        )
        {
            let latch = Arc::new(CountdownEvent::new(self.threads));
            println!("Shutting down {} threads", self.threads);
            for _ in 0..self.threads {
                self.sender.send(JobMsg::Stop(latch.clone()));
            }
            let timeout = Duration::from_millis(5000);
            let remaining = latch.wait_timeout(timeout);
            if remaining == 0 {
                println!("All threads shut down");
                Result::Ok(())
            } else {
                let msg = format!(
                    "Pool failed to shut down in time. {:?} threads remaining.",
                    remaining
                );
                Result::Err(String::from(msg))
            }
        } else {
            Result::Err(String::from("Pool is already shutting down!"))
        }
    }
}

struct CrossbeamPoolCore {
    sender: Sender<JobMsg>,
    receiver: Receiver<JobMsg>,
    shutdown: Arc<AtomicBool>,
    threads: usize,
}

//impl CrossbeamPoolCore {
//    fn new(sender: Sender<JobMsg>, receiver: Receiver<JobMsg>) -> CrossbeamPoolCore {
//        CrossbeamPoolCore { sender, receiver }
//    }
//}

impl Drop for CrossbeamPoolCore {
    fn drop(&mut self) {
        if !self.shutdown.compare_and_swap(
            false,
            true,
            Ordering::SeqCst,
        )
        {
            let latch = Arc::new(CountdownEvent::new(self.threads));
            println!("Shutting down {} threads", self.threads);
            for _ in 0..self.threads {
                self.sender.send(JobMsg::Stop(latch.clone()));
            }
            let timeout = Duration::from_millis(5000);
            let remaining = latch.wait_timeout(timeout);
            if remaining == 0 {
                println!("All threads shut down");
            } else {
                println!(
                    "Pool failed to shut down in time. {:?} threads remaining.",
                    remaining
                );
            }
        } else {
            println!("Pool is already shutting down!");
        }
    }
}

struct CrossbeamWorker {
    id: usize,
    recv: Receiver<JobMsg>,
}

impl CrossbeamWorker {
    fn new(id: usize, recv: Receiver<JobMsg>) -> CrossbeamWorker {
        CrossbeamWorker { id, recv }
    }
    fn id(&self) -> &usize {
        &self.id
    }
    fn run(&mut self) {
        println!("CrossbeamWorker {} starting", self.id);
        while let Ok(msg) = self.recv.recv() {
            match msg {
                JobMsg::Job(f) => f.call_box(),
                JobMsg::Stop(latch) => {
                    ignore(latch.decrement());
                    break;
                }
            }
        }
        println!("CrossbeamWorker {} shutting down", self.id);
    }
}

trait FnBox {
    fn call_box(self: Box<Self>);
}

impl<F: FnOnce()> FnBox for F {
    fn call_box(self: Box<F>) {
        (*self)()
    }
}

enum JobMsg {
    Job(Box<FnBox + Send + 'static>),
    Stop(Arc<CountdownEvent>),
}

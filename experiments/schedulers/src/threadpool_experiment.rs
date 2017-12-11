use super::*;

use threadpool::ThreadPool;

pub struct ThreadpoolExperiment;

struct ThreadpoolExperimentInstance {
    scheduler: ThreadPool,
    events: u64,
    latch: Arc<CountdownEvent>,
    in_parallelism: usize,
}

impl Experiment for ThreadpoolExperiment {
    fn setup(
        events: u64,
        in_parallelism: usize,
        out_threads: usize,
    ) -> Box<ExperimentInstance> {
        let inst = ThreadpoolExperimentInstance {
            scheduler: ThreadPool::new(out_threads),
            events,
            latch: Arc::new(CountdownEvent::new(in_parallelism)),
            in_parallelism,
        };
        Box::new(inst) as Box<ExperimentInstance>
    }
    fn label() -> String {
        String::from("threadpool::ThreadPool")
    }
}

impl ExperimentInstance for ThreadpoolExperimentInstance {
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
        ThreadpoolExperiment::label()
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

#![allow(clippy::unused_unit)]
use kompact::prelude::*;
use std::{env, fmt, ops::Range, sync::Arc};

struct Work {
    data: Arc<[u64]>,
    merger: fn(u64, &u64) -> u64,
    neutral: u64,
}
impl Work {
    fn with(data: Vec<u64>, merger: fn(u64, &u64) -> u64, neutral: u64) -> Self {
        let moved_data: Arc<[u64]> = data.into_boxed_slice().into();
        Work {
            data: moved_data,
            merger,
            neutral,
        }
    }
}
impl fmt::Debug for Work {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Work{{
            data=<data of length={}>,
            merger=<function>,
            neutral={}
        }}",
            self.data.len(),
            self.neutral
        )
    }
}

struct WorkPart {
    data: Arc<[u64]>,
    range: Range<usize>,
    merger: fn(u64, &u64) -> u64,
    neutral: u64,
}
impl WorkPart {
    fn from(work: &Work, range: Range<usize>) -> Self {
        WorkPart {
            data: work.data.clone(),
            range,
            merger: work.merger,
            neutral: work.neutral,
        }
    }
}
impl fmt::Debug for WorkPart {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "WorkPart{{
            data=<data of length={}>,
            range={:?},
            merger=<function>,
            neutral={}
        }}",
            self.data.len(),
            self.range,
            self.neutral
        )
    }
}

#[derive(Debug)]
struct WorkResult(u64);

// ANCHOR: manager_message
#[derive(Debug)]
enum ManagerMessage {
    Work(Ask<Work, WorkResult>),
    Result(WorkResult),
}
// ANCHOR_END: manager_message
// ANCHOR: manager_state
#[derive(ComponentDefinition)]
struct Manager {
    ctx: ComponentContext<Self>,
    num_workers: usize,
    workers: Vec<Arc<Component<Worker>>>,
    worker_refs: Vec<ActorRefStrong<WithSender<WorkPart, ManagerMessage>>>,
    outstanding_request: Option<Ask<Work, WorkResult>>,
    result_accumulator: Vec<u64>,
}
// ANCHOR_END: manager_state
impl Manager {
    fn new(num_workers: usize) -> Self {
        Manager {
            ctx: ComponentContext::uninitialised(),
            num_workers,
            workers: Vec::with_capacity(num_workers),
            worker_refs: Vec::with_capacity(num_workers),
            outstanding_request: None,
            result_accumulator: Vec::with_capacity(num_workers + 1),
        }
    }
}

impl ComponentLifecycle for Manager {
    fn on_start(&mut self) -> Handled {
        // set up our workers
        for _i in 0..self.num_workers {
            let worker = self.ctx.system().create(Worker::new);
            let worker_ref = worker.actor_ref().hold().expect("live");
            self.ctx.system().start(&worker);
            self.workers.push(worker);
            self.worker_refs.push(worker_ref);
        }
        Handled::Ok
    }

    fn on_stop(&mut self) -> Handled {
        // clean up after ourselves
        self.worker_refs.clear();
        let system = self.ctx.system();
        self.workers.drain(..).for_each(|worker| {
            system.stop(&worker);
        });
        Handled::Ok
    }

    fn on_kill(&mut self) -> Handled {
        self.on_stop()
    }
}

// ANCHOR: manager_actor
impl Actor for Manager {
    type Message = ManagerMessage;

    fn receive_local(&mut self, msg: Self::Message) -> Handled {
        match msg {
            ManagerMessage::Work(msg) => {
                assert!(
                    self.outstanding_request.is_none(),
                    "One request at a time, please!"
                );
                let work: &Work = msg.request();
                if self.num_workers == 0 {
                    // manager gotta work itself -> very unhappy manager
                    let res = work.data.iter().fold(work.neutral, work.merger);
                    msg.reply(WorkResult(res)).expect("reply");
                } else {
                    let len = work.data.len();
                    let stride = len / self.num_workers;
                    let mut start = 0usize;
                    let mut index = 0;
                    while start < len && index < self.num_workers {
                        let end = len.min(start + stride);
                        let range = start..end;
                        info!(self.log(), "Assigning {:?} to worker #{}", range, index);
                        let msg = WorkPart::from(work, range);
                        let worker = &self.worker_refs[index];
                        worker.tell(WithSender::from(msg, self));
                        start += stride;
                        index += 1;
                    }
                    if start < len {
                        // manager just does the rest itself
                        let res = work.data[start..len].iter().fold(work.neutral, work.merger);
                        self.result_accumulator.push(res);
                    } else {
                        // just put a neutral element in there, so our count is right in the end
                        self.result_accumulator.push(work.neutral);
                    }
                    self.outstanding_request = Some(msg);
                }
            }
            ManagerMessage::Result(msg) => {
                if self.outstanding_request.is_some() {
                    self.result_accumulator.push(msg.0);
                    if self.result_accumulator.len() == (self.num_workers + 1) {
                        let ask = self.outstanding_request.take().expect("ask");
                        let work: &Work = ask.request();
                        let res = self
                            .result_accumulator
                            .iter()
                            .fold(work.neutral, work.merger);
                        self.result_accumulator.clear();
                        let reply = WorkResult(res);
                        ask.reply(reply).expect("reply");
                    }
                } else {
                    error!(
                        self.log(),
                        "Got a response without an outstanding promise: {:?}", msg
                    );
                }
            }
        }
        Handled::Ok
    }

    fn receive_network(&mut self, _msg: NetMessage) -> Handled {
        unimplemented!("Still ignoring networking stuff.");
    }
}
// ANCHOR_END: manager_actor

#[derive(ComponentDefinition)]
struct Worker {
    ctx: ComponentContext<Self>,
}
impl Worker {
    fn new() -> Self {
        Worker {
            ctx: ComponentContext::uninitialised(),
        }
    }
}
ignore_lifecycle!(Worker);

// ANCHOR: worker_actor
impl Actor for Worker {
    type Message = WithSender<WorkPart, ManagerMessage>;

    fn receive_local(&mut self, msg: Self::Message) -> Handled {
        let my_slice = &msg.data[msg.range.clone()];
        let res = my_slice.iter().fold(msg.neutral, msg.merger);
        msg.reply(ManagerMessage::Result(WorkResult(res)));
        Handled::Ok
    }

    fn receive_network(&mut self, _msg: NetMessage) -> Handled {
        unimplemented!("Still ignoring networking stuff.");
    }
}
// ANCHOR_END: worker_actor

pub fn main() {
    let args: Vec<String> = env::args().collect();
    assert_eq!(
        3,
        args.len(),
        "Invalid arguments! Must give number of workers and size of the data array."
    );
    let num_workers: usize = args[1].parse().expect("number");
    let data_size: usize = args[2].parse().expect("number");
    run_task(num_workers, data_size);
}

fn run_task(num_workers: usize, data_size: usize) {
    let system = KompactConfig::default().build().expect("system");
    let manager = system.create(move || Manager::new(num_workers));
    system.start(&manager);
    let manager_ref = manager.actor_ref().hold().expect("live");

    let data: Vec<u64> = (1..=data_size).map(|v| v as u64).collect();
    let work = Work::with(data, overflowing_sum, 0u64);
    println!("Sending request...");
    // ANCHOR: main_ask
    let res = manager_ref
        .ask_with(|promise| ManagerMessage::Work(Ask::new(promise, work)))
        .wait();
    // ANCHOR_END: main_ask
    println!("*******\nGot result: {}\n*******", res.0);
    assert_eq!(triangular_number(data_size as u64), res.0);
    system.shutdown().expect("shutdown");
}

fn triangular_number(n: u64) -> u64 {
    (n * (n + 1u64)) / 2u64
}

fn overflowing_sum(lhs: u64, rhs: &u64) -> u64 {
    lhs.overflowing_add(*rhs).0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_workers() {
        run_task(3, 1000);
    }
}

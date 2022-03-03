

use super::*;
use executors::*;
use crate::{
    runtime::*,
};
use std::{
    collections::{
        HashMap,
        VecDeque
    },
    rc::Rc,
    cell::RefCell,
    future::Future,
    net::SocketAddr,
    ops::Deref,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    task::{Context, Poll},
    time::Duration,
};
use log::{
    debug,
    warn,
};
use crossbeam_channel::unbounded;
use async_task::*;

#[derive(Clone)]
struct SimulationScheduler(Rc<RefCell<SimulationSchedulerData>>);

unsafe impl Sync for SimulationScheduler {}
unsafe impl Send for SimulationScheduler {}

pub struct SimulationSchedulerData {
    queue: VecDeque<Arc<dyn CoreContainer>>,
} 

impl SimulationSchedulerData {
    pub fn new() -> SimulationSchedulerData {
        SimulationSchedulerData {
            queue: VecDeque::new(),
        }
    }

    pub fn simulate_step(&mut self) -> SchedulingDecision {
        match self.queue.pop_front() {
            Some(work) => work.execute(),
            None => SchedulingDecision::NoWork
        }
    }

    pub fn simulate_to_completion(&mut self) {
        while let Some(work) = self.queue.pop_front() {
            println!("doing work");
        }
    }
}

impl Scheduler for SimulationScheduler {
    fn schedule(&self, c: Arc<dyn CoreContainer>) -> () {
        println!("schedule");
        let mut data_ref = self.0.as_ref().borrow_mut();
        data_ref.queue.push_back(c);
    }

    fn shutdown_async(&self) -> (){
        println!("shutdown_async");
        todo!();
    }

    fn shutdown(&self) -> Result<(), String>{
        println!("shutdown");
        todo!();
    }

    fn box_clone(&self) -> Box<dyn Scheduler>{
        println!("box_clone");
        Box::new(self.clone()) as Box<dyn Scheduler>
    }

    fn poison(&self) -> (){
        println!("poison");
        todo!();
    }

    fn spawn(&self, future: futures::future::BoxFuture<'static, ()>) -> (){
        println!("spawn");
        todo!();
    }
}

fn maybe_reschedule(c: Arc<dyn CoreContainer>) {

    match c.execute() {
        SchedulingDecision::Schedule => {
            /*
            if cfg!(feature = "use_local_executor") {
                let res = try_execute_locally(move || maybe_reschedule(c));
                assert!(!res.is_err(), "Only run with Executors that can support local execute or remove the avoid_executor_lookups feature!");
            } else {
                let c2 = c.clone();
                c.system().schedule(c2);
            }*/
        }
        SchedulingDecision::Resume => maybe_reschedule(c),
        _ => (),
    }
}

pub struct SimulationScenario {
    systems: Vec<KompactSystem>,
    scheduler: SimulationScheduler,
}

impl SimulationScenario {
    pub fn new() -> SimulationScenario {
        SimulationScenario {
            systems: Vec::new(),
            scheduler: SimulationScheduler(Rc::new(RefCell::new(SimulationSchedulerData{
                queue: VecDeque::new(),
            }))),
        }
    }

    pub fn spawn_system(&mut self, cfg: KompactConfig) -> KompactSystem{
        let mut mut_cfg = cfg;

        let scheduler = self.scheduler.clone();
        mut_cfg.scheduler(move |_| Box::new(scheduler.clone()));
        mut_cfg.build().expect("system")
    }
}
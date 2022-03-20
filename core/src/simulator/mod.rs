

use super::*;
use executors::*;
use crate::{
    runtime::*,
    timer::{
        timer_manager::TimerRefFactory,

    }
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
    thread::{
        current,
    },
    mem::drop,
};
use log::{
    debug,
    warn,
};
use crossbeam_channel::unbounded;
use async_task::*;

use std::io::{stdin,stdout,Write};

use config_keys::*;

use backtrace::Backtrace;


#[derive(Clone)]
struct SimulationScheduler(Rc<RefCell<SimulationSchedulerData>>);

unsafe impl Sync for SimulationScheduler {}
unsafe impl Send for SimulationScheduler {}

pub struct SimulationSchedulerData {
    setup: bool,
    queue: VecDeque<Arc<dyn CoreContainer>>,
} 

impl SimulationSchedulerData {
    pub fn new() -> SimulationSchedulerData {
        SimulationSchedulerData {
            setup: true, 
            queue: VecDeque::new(),
        }
    }
}

impl Scheduler for SimulationScheduler {
    fn schedule(&self, c: Arc<dyn CoreContainer>) -> () {
        match current().name() {
            None => println!("No thread name"),
            Some(thread_name) => {
                println!("Thread name: {}", thread_name)
            },
        }

        println!("Component Definition Type Name: {}", c.type_name());

        if self.0.as_ref().borrow().setup {
            c.execute();
        } else {
            self.0.as_ref().borrow_mut().queue.push_back(c);
        }
    }

    fn shutdown_async(&self) -> (){
        println!("TODO shutdown_async");
        todo!();
    }

    fn shutdown(&self) -> Result<(), String>{
        println!("TODO shutdown");
        todo!();
    }

    fn box_clone(&self) -> Box<dyn Scheduler>{
        println!("TODO box_clone");
        Box::new(self.clone()) as Box<dyn Scheduler>
    }

    fn poison(&self) -> (){
        println!("TODO poison");
        todo!();
    }

    fn spawn(&self, future: futures::future::BoxFuture<'static, ()>) -> (){
        println!("TODO spawn");
        todo!();
    }
}

struct SimulationTimer(Rc<RefCell<SimulationTimerData>>);

unsafe impl Sync for SimulationTimer {}
unsafe impl Send for SimulationTimer {}

struct SimulationTimerData{
    inner: timer::SimulationTimer,
}

impl SimulationTimerData {
    pub(crate) fn new() -> SimulationTimerData {
        SimulationTimerData {
            inner: timer::SimulationTimer::new(),
        }
    }
}

impl TimerRefFactory for SimulationTimer {
    fn timer_ref(&self) -> timer::TimerRef {
        self.0.as_ref().borrow_mut().inner.timer_ref()
    }
}

impl TimerComponent for SimulationTimer{
    fn shutdown(&self) -> Result<(), String> {
        todo!();
    }
}

impl SimulationTimer {
    pub(crate) fn new_timer_component() -> Box<dyn TimerComponent> {
        let t = SimulationTimer(Rc::new(RefCell::new(SimulationTimerData::new())));
        Box::new(t) as Box<dyn TimerComponent>
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
            scheduler: SimulationScheduler(Rc::new(RefCell::new(SimulationSchedulerData::new()))),
        }
    }

    pub fn spawn_system(&mut self, cfg: KompactConfig) -> KompactSystem {
        let mut mut_cfg = cfg;
        KompactConfig::set_config_value(&mut mut_cfg, &config_keys::system::THREADS, 1);
        let scheduler = self.scheduler.clone();
        mut_cfg.scheduler(move |_| Box::new(scheduler.clone()));      
        mut_cfg.timer::<SimulationTimer, _>(SimulationTimer::new_timer_component);
        mut_cfg.build().expect("system")
    }

    pub fn end_setup(&mut self) -> () {
        let mut data_ref = self.scheduler.0.as_ref().borrow_mut();
        data_ref.setup = false;
    }

    fn get_work(&mut self) -> Option<Arc<dyn CoreContainer>> {
        let mut data_ref = self.scheduler.0.as_ref().borrow_mut();
        data_ref.queue.pop_front()
    }

    pub fn simulate_step(&mut self) -> SchedulingDecision {
        match self.get_work(){
            Some(w) => w.execute(),
            None => SchedulingDecision::NoWork
        }
    }

    pub fn simulate_to_completion(&mut self) {
        loop {
            self.simulate_step();
        }
    }
}


use self::{simulation_network::SimulationNetwork, simulation_network_dispatcher::{SimulationNetworkConfig, SimulationNetworkDispatcher}};

use super::*;
use arc_swap::ArcSwap;
use executors::*;
use messaging::{DispatchEnvelope, RegistrationResult};
use prelude::{NetworkConfig, NetworkStatusPort};
use rustc_hash::FxHashMap;
pub mod simulation_network_dispatcher;
pub mod simulation_bridge;
pub mod simulation_network;
use crate::{
    runtime::*,
    timer::{
        timer_manager::TimerRefFactory,

    }, net::{ConnectionState, buffers::BufferChunk}, lookup::ActorStore, dispatch::queue_manager::QueueManager, serde_serialisers::Serde
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

type SimulationNetHashMap<K, V> = FxHashMap<K, V>;

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

        //println!("Component Definition Type Name: {}", c.type_name());

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

#[derive(Clone)]
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

/* impl SimulationTimer {
    pub(crate) fn new_timer_component() -> Box<dyn TimerComponent> {
        let t = SimulationTimer(Rc::new(RefCell::new(SimulationTimerData::new())));
        Box::new(t) as Box<dyn TimerComponent>
    }
}*/
/* 
#[derive(ComponentDefinition)]
pub struct SimulationNetworkDispatcher {
    ctx: ComponentContext<SimulationNetworkDispatcher>,
    /// Local map of connection statuses
    connections: SimulationNetHashMap<SocketAddr, ConnectionState>,
    /// Network configuration for this dispatcher
    cfg: NetworkConfig,
    /// Shared lookup structure for mapping [actor paths](ActorPath) and [actor refs](ActorRef)
    lookup: Arc<ArcSwap<ActorStore>>,
    // Fields initialized at [Start](ControlEvent::Start) – they require ComponentContextual awareness
    /// Bridge into asynchronous networking layer
    net_bridge: Option<net::Bridge>,
    /// A cached version of the bound system path
    system_path: Option<SystemPath>,
    /// Management for queuing Frames during network unavailability (conn. init. and MPSC unreadiness)
    queue_manager: QueueManager,
    /// Reaper which cleans up deregistered actor references in the actor lookup table
    reaper: lookup::gc::ActorRefReaper,
    notify_ready: Option<KPromise<()>>,
    /// Stores the number of retry-attempts for connections. Checked and incremented periodically by the reaper.
    retry_map: FxHashMap<SocketAddr, u8>,
    garbage_buffers: VecDeque<BufferChunk>,
    /// The dispatcher emits NetworkStatusUpdates to the `NetworkStatusPort`.
    network_status_port: ProvidedPort<NetworkStatusPort>,
}

impl NetworkActor for SimulationNetworkDispatcher{
    type Message = DispatchEnvelope;

    type Deserialiser = Serde;

    fn receive(&mut self, sender: Option<ActorPath>, msg: Self::Message) -> Handled {
        todo!()
    }

    fn on_error(&mut self, error: prelude::UnpackError<prelude::NetMessage>) -> Handled {
        warn!(
            "{} {} {} {}", self.log(),
            "Could not deserialise a message with Deserialiser with id={}. Error was: {:?}",
            Self::Deserialiser::SER_ID,
            error
        );
        Handled::Ok
    }
}

impl ComponentLifecycle for SimulationNetworkDispatcher {
    fn on_start(&mut self) -> Handled
    where
        Self: 'static,
    {
        debug!("{} {}", self.log(), "Starting...");
        Handled::Ok
    }

    fn on_stop(&mut self) -> Handled
    where
        Self: 'static,
    {
        debug!("{} {}", self.log(), "Stopping...");
        Handled::Ok
    }

    fn on_kill(&mut self) -> Handled
    where
        Self: 'static,
    {
        debug!("{} {}", self.log(), "Killing...");
        Handled::Ok
    }
}*/


pub struct SimulationScenario {
    systems: Vec<KompactSystem>,
    scheduler: SimulationScheduler,
    timer: SimulationTimer,
    network: Arc<Mutex<SimulationNetwork>>
}

impl SimulationScenario {
    pub fn new() -> SimulationScenario {
        SimulationScenario {
            systems: Vec::new(),
            scheduler: SimulationScheduler(Rc::new(RefCell::new(SimulationSchedulerData::new()))),
            timer: SimulationTimer(Rc::new(RefCell::new(SimulationTimerData::new()))),
            //network: Rc::new(RefCell::new(SimulationNetwork::new()))
            network: Arc::new(Mutex::new(SimulationNetwork::new()))
        }
    }

    pub fn spawn_system(&mut self, cfg: KompactConfig) -> KompactSystem {
        let mut mut_cfg = cfg;
        KompactConfig::set_config_value(&mut mut_cfg, &config_keys::system::THREADS, 1);
        let scheduler = self.scheduler.clone();
        mut_cfg.scheduler(move |_| Box::new(scheduler.clone()));
        let timer = self.timer.clone();
        mut_cfg.timer::<SimulationTimer, _>(move || Box::new(timer.clone()));
        let dispatcher = SimulationNetworkConfig::default().build(self.network.clone());
        mut_cfg.system_components(DeadletterBox::new, dispatcher);
        let system = mut_cfg.build().expect("system");
        system
    }

    pub fn end_setup(&mut self) -> () {
        let mut data_ref = self.scheduler.0.as_ref().borrow_mut();
        data_ref.setup = false;
    }

    fn get_work(&mut self) -> Option<Arc<dyn CoreContainer>> {
        let mut data_ref = self.scheduler.0.as_ref().borrow_mut();
        data_ref.queue.pop_front()
    }

    pub fn create_and_register<C, F>(
        &self,
        sys: KompactSystem,
        f: F,
    ) -> (Arc<Component<C>>, KFuture<RegistrationResult>)
    where
        F: FnOnce() -> C,
        C: ComponentDefinition + 'static,
    {
        sys.create_and_register(f)
    }

    /*fn register_system_to_simulated_network(&mut self, dispatcher: SimulationNetworkDispatcher) -> () {
        self.network.lock().unwrap().register_system(dispatcher., actor_store)
    }*/

    pub fn get_simulated_network(&self) -> Arc<Mutex<SimulationNetwork>>{
        self.network.clone()
    }

    pub fn simulate_step(&mut self) -> SchedulingDecision {
        match self.get_work(){
            Some(w) => {
                //println!("EXECUTING SOMETHING!!!!!!!!!!!!!!!!");
                w.execute()
            },
            None => SchedulingDecision::NoWork
        }
    }

    pub fn simulate_to_completion(&mut self) {
        loop {
            self.simulate_step();
        }
    }
}
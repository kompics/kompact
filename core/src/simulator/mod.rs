pub mod adaptor;
pub mod core;
pub mod events;
pub mod instrumentation;
pub mod network;
pub mod result;
pub mod run;
pub mod scheduler;
pub mod stochastic;
pub mod util;

pub use crate::{
    serialisation::{
        serialisation_ids, 
        Deserialiser, 
        SerError, 
        SerId, 
        Serialisable
    },
};

use rand::prelude::*;
use rand_chacha::ChaCha20Rng;

//use lazy_static::lazy_static;

use std::collections::VecDeque;
//use std::sync::Mutex;

pub use bytes::{Buf, BufMut};
pub use std::{
    any::Any,
    convert::{From, Into},
};

use stochastic::events::*;


#[derive(Debug, Clone)]
struct SimulationScenario {
    processes: VecDeque<StochasticProcess>,
    processCount: u32,
    terminatedEvent: Option<StochasticSimulationTerminatedEvent>,
    rng: ChaCha20Rng,
}

#[derive(Debug, Clone)]
struct StochasticProcess {
    relativeStartTime: bool,
    startTime: u64,
    startEvent: StochasticProcessStartEvent,
    terminateEvent: StochasticProcessTerminatedEvent,
    stochasticEvent: StochasticProcessEvent,
}

/*
impl Serialisable for SimulationScenario {
    fn ser_id(&self) -> SerId {
        todo!();
    }

    fn size_hint(&self) -> Option<usize> {
        todo!();
    }

    fn serialise(&self, buf: &mut dyn BufMut) -> Result<(), SerError> {
        todo!();
    }

    fn local(self: Box<Self>) -> Result<Box<dyn Any + Send>, Box<dyn Serialisable>> {
        todo!();
    }
}

impl Serialisable for StochasticProcess {
    fn ser_id(&self) -> SerId {
        todo!();
    }

    fn size_hint(&self) -> Option<usize> {
        todo!();
    }

    fn serialise(&self, buf: &mut dyn BufMut) -> Result<(), SerError> {
        todo!();
    }

    fn local(self: Box<Self>) -> Result<Box<dyn Any + Send>, Box<dyn Serialisable>> {
        todo!();
    }
}*/

impl SimulationScenario {
    // get-random?

    pub fn new(init_seed: u64) -> Self {
        SimulationScenario {
            processes: VecDeque::new(),
            processCount: 0,
            terminatedEvent: Option::None,
            rng: ChaCha20Rng::seed_from_u64(init_seed),
        }
    }

    pub fn set_seed(&mut self, new_seed: u64){
        self.rng = ChaCha20Rng::seed_from_u64(new_seed);
    }
}


/*
const INIT_SEED: u64 = 0;

lazy_static!{
    static ref SEED: Mutex<u64> = Mutex::new(INIT_SEED);
    //TODO: Using chacha and not StdRng because its deterministic across platforms
    static ref RANDOM: Mutex<ChaCha20Rng> = Mutex::new(ChaCha20Rng::seed_from_u64(INIT_SEED));
}

impl SimulationScenario {
    const serialVersionUID: SerId = 0; // What to put this to?
    
    pub fn set_seed(new_seed: u64){
        let mut seed_ref = SEED.lock().unwrap();
        *seed_ref = new_seed;
        let mut random_ref = RANDOM.lock().unwrap();
        *random_ref = ChaCha20Rng::seed_from_u64(new_seed);
    }

    // get-random?

    pub fn new() -> Self {
        SimulationScenario {
            processes: LinkedList::new(),
            processCount: 0,
            terminatedEvent: Option::None,
        }
    }
}
*/



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

use lazy_static::lazy_static;

use std::collections::LinkedList;
use std::sync::Mutex;

pub use bytes::{Buf, BufMut};
pub use std::{
    any::Any,
    convert::{From, Into},
};

use stochastic::events::*;


#[derive(Debug, Clone)]
struct SimulationScenario {
    processes: LinkedList<StochasticProcess>,
    processCount: u32,
    terminatedEvent: Option<StochasticSimulationTerminatedEvent>,
}

#[derive(Debug, Clone)]
struct StochasticProcess {

}

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
}

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


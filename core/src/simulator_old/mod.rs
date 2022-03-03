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
    simulator::{
        adaptor::{
            distributions::*,
        }
    }
};

use rand::prelude::*;
use rand::{
    Rng,
    rngs::{StdRng},
    distributions::{
        Distribution,
        Uniform,
    }
};

//use rand_chacha::ChaCha20Rng;

use lazy_static::lazy_static;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

pub use bytes::{Buf, BufMut};
pub use std::{
    any::Any,
    convert::{From, Into},
};

use stochastic::events::*;

const INIT_SEED: u64 = 0;

lazy_static!{
    static ref SEED: Mutex<u64> = Mutex::new(INIT_SEED);
    static ref STATIC_RNG: Arc<Mutex<StdRng>> = Arc::new(Mutex::new(StdRng::seed_from_u64(INIT_SEED)));
}

#[derive(Debug, Clone)]
struct SimulationScenario {
    processes: VecDeque<StochasticProcess>,
    processCount: u32,
    terminatedEvent: Option<StochasticSimulationTerminatedEvent>,
}

#[derive(Debug, Clone)]
struct StochasticProcess {
    relativeStartTime: bool,
    startTime: u64,
    startEvent: StochasticProcessStartEvent,
    terminateEvent: StochasticProcessTerminatedEvent,
    stochasticEvent: StochasticProcessEvent,
}

impl SimulationScenario {
    // get-random?

    pub fn new(init_seed: u64) -> Self {
        SimulationScenario {
            processes: VecDeque::new(),
            processCount: 0,
            terminatedEvent: Option::None,
        }
    }

    pub fn set_seed(new_seed: u64){
        let mut seed_ref = SEED.lock().unwrap();
        *seed_ref = new_seed;
        let mut random_ref = STATIC_RNG.lock().unwrap();
        *random_ref = StdRng::seed_from_u64(new_seed);
    }

    pub fn get_distribution<D: Distribution<i32>>(&mut self, distribution: D) -> SimulatorDistribution<D> {
        SimulatorDistribution::new(distribution, Arc::clone(&STATIC_RNG))
    }
}

#[cfg(test)]
mod tests {
    pub use crate::{
        simulator::*,
    };

    #[test]
    fn uniform_distribution_test() {

        let mut reproducible_sequence1 = Vec::new();
        let mut reproducible_sequence2 = Vec::new();
        
        let r1 = 0..10;
        let r2 = 10..20;

        {
            let mut ss : SimulationScenario = SimulationScenario::new(1234);
            let u1 = ss.get_distribution(Uniform::from(0..10));
            let u2 = ss.get_distribution(Uniform::from(10..20));
    
            for _ in 0..10 {
                let throw1 = u1.draw();
                let throw2 = u2.draw();

                reproducible_sequence1.push(throw1);
                reproducible_sequence2.push(throw2);
                
                assert!(r1.contains(&throw1));
                assert!(r2.contains(&throw2));
            }
        }

        let mut ss : SimulationScenario = SimulationScenario::new(1234);
        let u1 = ss.get_distribution(Uniform::from(0..10));
        let u2 = ss.get_distribution(Uniform::from(10..20));

        for i in 0..10 {
            let throw1 = u1.draw();
            let throw2 = u2.draw();
            
            assert!(r1.contains(&throw1));
            assert!(r2.contains(&throw2));

            assert_eq!(reproducible_sequence1[i], throw1);
            assert_eq!(reproducible_sequence2[i], throw2);
        }
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
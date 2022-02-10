pub mod extra;

pub use crate::{
    simulator::*,
};

use std::cmp;
use std::ops::DerefMut;
use rand::{
    Rng,
    rngs::{StdRng},
    distributions::{
        Distribution,
        Uniform,
    }
};

use std::sync::{Arc, Mutex};

pub struct SimulatorDistribution<D>{
    distribution: D,
    rng: Arc<Mutex<StdRng>>,
}

impl <D: Distribution<i32>> SimulatorDistribution<D>{
    pub fn new(distribution: D, rng: Arc<Mutex<StdRng>>) -> Self {
        SimulatorDistribution{
            distribution: distribution,
            rng: rng,
        }
    }

    pub fn draw(&self) -> i32 {
        self.distribution.sample(self.rng.lock().unwrap().deref_mut())
    }
}

/*
pub mod extra;

pub use crate::{
    simulator::*,
};

use std::cmp;
use std::ops::DerefMut;
use rand::{
    Rng,
    rngs::{StdRng},
    distributions::{
        Distribution,
        Uniform,
    }
};

use std::sync::{Arc, Mutex};


pub struct UniformDistribution{
    distribution: Uniform<u64>,
    rng: Arc<Mutex<StdRng>>,
}

impl UniformDistribution{
    pub fn new(min: u64, max: u64, rng: Arc<Mutex<StdRng>>) -> Self {
        UniformDistribution{
            distribution: Uniform::from(min..max),
            rng: rng,
        }
    }

    fn draw(&self) -> u64 {
        self.distribution.sample(STATIC_RNG.lock().unwrap().deref_mut())
    }
}

impl Distribution<u64> for Uniform {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> u64 {
        // Generate a new event
        3
    }
}
*/

/*
//Currently distributions only support one number type which is u64. Might have to make this generic for several types in the future. 
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DistributionType {
    Constant,
    Uniform, 
    Normal,
    Exponential,
    Other,
}
}

// UniformDistribution (Long)
*/
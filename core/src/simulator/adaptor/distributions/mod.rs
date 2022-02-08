pub mod extra;

use std::cmp;
use rand::prelude::*;
use rand_chacha::ChaCha20Rng;

//Currently distributions only support one number type which is u64. Might have to make this generic for several types in the future. 
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DistributionType {
    Constant,
    Uniform, 
    Normal,
    Exponential,
    Other,
} 

pub trait Distribution {
    fn getType(&self) -> DistributionType;
    fn draw(&mut self) -> u64;
}

// ConstantDistribution (Long)

struct ConstantDistribution {
    value: u64,
    distribution_type: DistributionType,
}

impl ConstantDistribution {
    pub fn new(val: u64) -> Self {
        ConstantDistribution{
            value: val,
            distribution_type: DistributionType::Constant,
        }
    }
}

impl Distribution for ConstantDistribution{
    fn getType(&self) -> DistributionType {
        self.distribution_type
    }

    fn draw(&mut self) -> u64 {
        self.value
    }
}

// UniformDistribution (Long)

struct UniformDistribution<'a>{
    min: u64,
    max: u64,
    rng: &'a mut ChaCha20Rng,
    distribution_type: DistributionType,
}

impl<'a> UniformDistribution<'a> {
    pub fn new(min: u64, max: u64, rng: &'a mut ChaCha20Rng) -> Self {
        UniformDistribution{
            min: cmp::min(min, max),
            max: cmp::max(min, max),
            rng: rng,
            distribution_type: DistributionType::Constant,
        }
    }
}

impl<'a> Distribution for UniformDistribution<'a>{
    fn getType(&self) -> DistributionType {
        self.distribution_type
    }

    fn draw(&mut self) -> u64 {
        let mut random_number: u64 = self.rng.gen();
        random_number *= (self.max - self.min);
        random_number += self.min;
        random_number
    }
}
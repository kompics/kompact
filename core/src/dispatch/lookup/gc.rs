use crossbeam::sync::ArcCell;
use crate::dispatch::lookup::ActorLookup;
use std::sync::Arc;

mod defaults {
    use super::FeedbackAlgorithm;

    pub const INITIAL_INTERVAL_MS: u64 = 500;
    pub const MIN_INTERVAL_MS: u64 = 20;
    pub const MAX_INTERVAL_MS: u64 = 5_000;
    pub const ADDITIVE_INCREASE: u64 = 500;
    //pub const MULTIPLICATIVE_DECREASE_NUM: u64 = 1;
    pub const MULTIPLICATIVE_DECREASE_DEN: u64 = 2;
    pub const ALGORITHM: FeedbackAlgorithm = super::FeedbackAlgorithm::AIMD;
}

/// Reaps deallocated actor references from the underlying lookup table.
pub struct ActorRefReaper {
    scheduled: bool,
    strategy: UpdateStrategy,
}

/// Defines an interval update strategy.
pub struct UpdateStrategy {
    curr: u64,
    incr: u64,
    decr: u64,
    min: u64,
    max: u64,
    algo: FeedbackAlgorithm,
}

pub enum FeedbackAlgorithm {
    AIMD, // additive increase/multiplicative decrease
    MIMD, // multiplicative increase/multiplicative decrease
}

impl Default for ActorRefReaper {
    fn default() -> Self {
        ActorRefReaper {
            scheduled: false,
            strategy: UpdateStrategy::new(
                defaults::INITIAL_INTERVAL_MS,
                defaults::MIN_INTERVAL_MS,
                defaults::MAX_INTERVAL_MS,
                defaults::ADDITIVE_INCREASE,
                defaults::MULTIPLICATIVE_DECREASE_DEN,
                defaults::ALGORITHM,
            ),
        }
    }
}

impl ActorRefReaper {
    pub fn new(
        initial_interval: u64,
        min: u64,
        max: u64,
        incr: u64,
        decr: u64,
        algo: FeedbackAlgorithm,
    ) -> Self {
        ActorRefReaper {
            scheduled: false,
            strategy: UpdateStrategy::new(initial_interval, min, max, incr, decr, algo),
        }
    }

    /// Walks through all stored [ActorRef] entries and removes
    /// the ones which have been deallocated, returning the # of removed instances.
    pub fn run<T: ActorLookup>(&self, table: &Arc<ArcCell<T>>) -> usize {
        let new_table = table.get();
        let mut new_table = (*new_table).clone();
        let removed = new_table.cleanup();
        table.set(Arc::new(new_table));
        removed
    }

    pub fn strategy(&self) -> &UpdateStrategy {
        &self.strategy
    }

    pub fn strategy_mut(&mut self) -> &mut UpdateStrategy {
        &mut self.strategy
    }

    pub fn schedule(&mut self) {
        self.scheduled = true;
    }

    pub fn is_scheduled(&self) -> bool {
        self.scheduled
    }
}

impl UpdateStrategy {
    pub fn new(
        curr: u64,
        min: u64,
        max: u64,
        incr: u64,
        decr: u64,
        algo: FeedbackAlgorithm,
    ) -> Self {
        UpdateStrategy {
            curr,
            incr,
            decr,
            min,
            max,
            algo,
        }
    }

    pub fn curr(&self) -> u64 {
        self.curr
    }

    /// Increments the current value, clamping it at the configured maximum.
    /// The updated value is returned.
    pub fn incr(&mut self) -> u64 {
        use self::FeedbackAlgorithm::*;
        self.curr = match self.algo {
            AIMD => self.curr + self.incr,
            MIMD => self.curr * self.incr,
        }
        .min(self.max);
        self.curr
    }

    /// Decrements the current value, clamping it at the configured minimum.
    /// The updated value is returned.
    pub fn decr(&mut self) -> u64 {
        use self::FeedbackAlgorithm::*;
        self.curr = match self.algo {
            AIMD | MIMD => self.curr / self.decr,
        }
        .max(self.min);
        self.curr
    }
}

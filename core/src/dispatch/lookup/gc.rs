use crate::{config::Config, dispatch::lookup::*};
use arc_swap::ArcSwap;

mod defaults {
    use super::FeedbackAlgorithm;

    pub const INITIAL_INTERVAL_MS: u64 = 500;
    pub const MIN_INTERVAL_MS: u64 = 20;
    pub const MAX_INTERVAL_MS: u64 = 5_000;
    pub const ADDITIVE_INCREASE: u64 = 500;
    //pub const MULTIPLICATIVE_DECREASE_NUM: u64 = 1;
    pub const MULTIPLICATIVE_DECREASE_DEN: u64 = 2;
    pub const ALGORITHM: FeedbackAlgorithm = super::FeedbackAlgorithm::Aimd;
}

/// Configuration keys for tuning actor-reference garbage collection.
pub mod config_keys {
    // TODO(Adam): Convert to the new config mechanism
    /// Selects the feedback algorithm used for reaper interval updates.
    pub const ALGORITHM: &str = "kompact.dispatch.actor-lookup.gc.algorithm";
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

/// Controls how the reaper adapts its next collection interval.
pub enum FeedbackAlgorithm {
    /// Additive increase and multiplicative decrease.
    Aimd,
    /// Multiplicative increase and multiplicative decrease.
    Mimd,
}

impl Default for ActorRefReaper {
    fn default() -> Self {
        Self::new(
            defaults::INITIAL_INTERVAL_MS,
            defaults::MIN_INTERVAL_MS,
            defaults::MAX_INTERVAL_MS,
            defaults::ADDITIVE_INCREASE,
            defaults::MULTIPLICATIVE_DECREASE_DEN,
            defaults::ALGORITHM,
        )
    }
}

impl ActorRefReaper {
    /// Creates a reaper with the provided interval update parameters.
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

    /// Builds a reaper from the Kompact configuration.
    pub fn from_config(conf: &Config) -> Self {
        // TODO(Adam): Make all parameters configurable
        let algorithm = match conf
            .select(config_keys::ALGORITHM)
            .as_string()
            .ok()
            .as_deref()
        {
            Some("AIMD") => FeedbackAlgorithm::Aimd,
            Some("MIMD") => FeedbackAlgorithm::Mimd,
            Some(s) => panic!(
                "Illegal configuration argument for key {}: {}. Allowed values are [AIMD, MIMD]",
                config_keys::ALGORITHM,
                s
            ),
            None => defaults::ALGORITHM,
        };
        Self::new(
            defaults::INITIAL_INTERVAL_MS,
            defaults::MIN_INTERVAL_MS,
            defaults::MAX_INTERVAL_MS,
            defaults::ADDITIVE_INCREASE,
            defaults::MULTIPLICATIVE_DECREASE_DEN,
            algorithm,
        )
    }

    /// Walks through all stored [ActorRef] entries and removes
    /// the ones which have been deallocated, returning the # of removed instances.
    pub fn run(&self, table: &ArcSwap<ActorStore>) -> usize {
        let mut removed = 0;
        table.rcu(|current| {
            let mut next = ActorStore::clone(current);
            removed = next.cleanup();
            next
        });
        removed
    }

    /// Returns the current interval update strategy.
    pub fn strategy(&self) -> &UpdateStrategy {
        &self.strategy
    }

    /// Returns the mutable interval update strategy.
    pub fn strategy_mut(&mut self) -> &mut UpdateStrategy {
        &mut self.strategy
    }

    /// Marks the reaper as scheduled.
    pub fn schedule(&mut self) {
        self.scheduled = true;
    }

    /// Returns whether the reaper is already scheduled.
    pub fn is_scheduled(&self) -> bool {
        self.scheduled
    }
}

impl UpdateStrategy {
    /// Creates a new interval update strategy.
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

    /// Returns the current interval in milliseconds.
    pub fn curr(&self) -> u64 {
        self.curr
    }

    /// Increments the current value, clamping it at the configured maximum.
    /// The updated value is returned.
    pub fn incr(&mut self) -> u64 {
        use self::FeedbackAlgorithm::*;
        self.curr = match self.algo {
            Aimd => self.curr + self.incr,
            Mimd => self.curr * self.incr,
        }
        .min(self.max);
        self.curr
    }

    /// Decrements the current value, clamping it at the configured minimum.
    /// The updated value is returned.
    pub fn decr(&mut self) -> u64 {
        use self::FeedbackAlgorithm::*;
        self.curr = match self.algo {
            Aimd | Mimd => self.curr / self.decr,
        }
        .max(self.min);
        self.curr
    }
}

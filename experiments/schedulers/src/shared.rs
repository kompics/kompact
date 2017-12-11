use super::*;

pub trait Experiment {
    fn setup(events: u64, in_parallelism: usize, out_threads: usize) -> Box<ExperimentInstance>;
    fn label() -> String;
}

pub trait ExperimentInstance {
    fn run(&mut self);
    fn label(&self) -> String;
}

#[inline(always)]
pub fn ignore<V>(_: V) -> () {
    ()
}

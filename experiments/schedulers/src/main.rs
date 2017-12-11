extern crate synchronoise;
extern crate threadpool;
extern crate time;
extern crate crossbeam_channel;
extern crate uuid;

mod shared;
mod threadpool_experiment;
mod crossbeam_experiment;

pub use shared::*;
pub use synchronoise::CountdownEvent;
pub use std::sync::Arc;
pub use std::thread;

use std::env;
use threadpool_experiment::ThreadpoolExperiment;
use crossbeam_experiment::CrossbeamExperiment;

const EVENTS: u64 = 2;
const NS_TO_S: f64 = 1.0 / (1000.0 * 1000.0 * 1000.0);

fn main() {
    let args: Vec<_> = env::args().collect();
    if args.len() != 3 {
        panic!("Expected parameters are 1: number of threads and 2: sending parallelism");
    }
    let threads: usize = args[1].parse().unwrap();
    let in_parallelism: usize = args[2].parse().unwrap();
    println!(
        "Starting Scheduler Experiment Suite with {} thread and in_parallelism={}",
        threads,
        in_parallelism
    );
    let total_events = (EVENTS * (in_parallelism as u64)) as f64;
    test(
        ThreadpoolExperiment::setup(EVENTS, in_parallelism, threads),
        total_events,
    );
    test(
        CrossbeamExperiment::setup(EVENTS, in_parallelism, threads),
        total_events,
    );
    //    let mut total: f64 = 0.0;
    //    let runs = 20;
    //    for i in 0..runs {
    //        println!("Run #{}", i);
    //        let res = test(
    //            CrossbeamExperiment::setup(EVENTS, in_parallelism, threads),
    //            total_events,
    //        );
    //        total += res;
    //    }
    //    let avg = total / (runs as f64);
    //    println!("Average {} run performance: {}schedulings/s", runs, avg);
}

fn test(mut exp: Box<ExperimentInstance>, total_events: f64) -> f64 {
    println!("Starting run for {} Experiment", exp.label());
    let startt = time::precise_time_ns();
    exp.run();
    let endt = time::precise_time_ns();
    println!("all done!");
    let difft = (endt - startt) as f64;
    let diffts = difft * NS_TO_S;
    let events_per_second = total_events / diffts;
    println!(
        "Experiment {} ran {}schedulings/s",
        exp.label(),
        events_per_second
    );
    events_per_second
}

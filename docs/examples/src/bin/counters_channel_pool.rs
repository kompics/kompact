#![allow(clippy::unused_unit)]
use kompact::{executors, prelude::*};
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq)]
struct CurrentCount {
    messages: u64,
    events: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct CountMe;

struct CounterPort;
impl Port for CounterPort {
    type Indication = CurrentCount;
    type Request = CountMe;
}

#[derive(ComponentDefinition)]
struct Counter {
    ctx: ComponentContext<Self>,
    counter_port: ProvidedPort<CounterPort>,
    msg_count: u64,
    event_count: u64,
}
impl Counter {
    pub fn new() -> Self {
        Counter {
            ctx: ComponentContext::uninitialised(),
            counter_port: ProvidedPort::uninitialised(),
            msg_count: 0u64,
            event_count: 0u64,
        }
    }

    fn current_count(&self) -> CurrentCount {
        CurrentCount {
            messages: self.msg_count,
            events: self.event_count,
        }
    }
}
impl ComponentLifecycle for Counter {
    fn on_start(&mut self) -> Handled {
        info!(self.ctx.log(), "Got a start event!");
        self.event_count += 1u64;
        Handled::Ok
    }

    fn on_stop(&mut self) -> Handled {
        info!(self.ctx.log(), "Got a stop event!");
        self.event_count += 1u64;
        Handled::Ok
    }

    fn on_kill(&mut self) -> Handled {
        info!(self.ctx.log(), "Got a kill event!");
        self.event_count += 1u64;
        Handled::Ok
    }
}
impl Provide<CounterPort> for Counter {
    fn handle(&mut self, _event: CountMe) -> Handled {
        info!(self.ctx.log(), "Got a counter event!");
        self.event_count += 1u64;
        self.counter_port.trigger(self.current_count());
        Handled::Ok
    }
}

impl Actor for Counter {
    type Message = Ask<CountMe, CurrentCount>;

    fn receive_local(&mut self, msg: Self::Message) -> Handled {
        msg.complete(|_request| {
            info!(self.ctx.log(), "Got a message!");
            self.msg_count += 1u64;
            self.current_count()
        })
        .expect("complete");
        Handled::Ok
    }

    fn receive_network(&mut self, _msg: NetMessage) -> Handled {
        unimplemented!("We are still ignoring network messages.");
    }
}

pub fn main() {
    let mut conf = KompactConfig::default();
    conf.threads(1usize);
    // ANCHOR: channel_pool
    conf.executor(executors::crossbeam_channel_pool::ThreadPool::new);
    // ANCHOR_END: channel_pool
    let system = conf.build().expect("system");
    let counter = system.create(Counter::new);
    system.start(&counter);
    let actor_ref = counter.actor_ref();
    let port_ref: ProvidedRef<CounterPort> = counter.provided_ref();
    for _i in 0..100 {
        let current_count = actor_ref.ask(Ask::of(CountMe)).wait();
        info!(system.logger(), "The current count is: {:?}", current_count);
    }
    for _i in 0..100 {
        system.trigger_r(CountMe, &port_ref);
        // Where do the answers go?
    }
    std::thread::sleep(Duration::from_millis(1000));
    let current_count = actor_ref.ask(Ask::of(CountMe)).wait();
    info!(system.logger(), "The final count is: {:?}", current_count);
    system.shutdown().expect("shutdown");
    // Wait a bit longer, so all output is logged (asynchronously) before shutting down
    std::thread::sleep(Duration::from_millis(10));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_counters() {
        main();
    }
}

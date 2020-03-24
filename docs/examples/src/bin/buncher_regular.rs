use kompact::prelude::*;
use kompact_examples::batching::*;
use std::time::Duration;

#[derive(ComponentDefinition, Actor)]
struct Buncher {
    ctx: ComponentContext<Self>,
    batch_port: ProvidedPort<Batching, Self>,
    batch_size: usize,
    timeout: Duration,
    current_batch: Vec<Ping>,
    outstanding_timeout: Option<ScheduledTimer>,
}

impl Buncher {
    fn new(batch_size: usize, timeout: Duration) -> Buncher {
        Buncher {
            ctx: ComponentContext::new(),
            batch_port: ProvidedPort::new(),
            batch_size,
            timeout,
            current_batch: Vec::with_capacity(batch_size),
            outstanding_timeout: None,
        }
    }

    fn trigger_batch(&mut self) -> () {
        let mut new_batch = Vec::with_capacity(self.batch_size);
        std::mem::swap(&mut new_batch, &mut self.current_batch);
        self.batch_port.trigger(Batch(new_batch))
    }

    fn handle_timeout(&mut self, timeout_id: ScheduledTimer) -> () {
        match self.outstanding_timeout {
            Some(ref timeout) if *timeout == timeout_id => {
                self.trigger_batch();
            }
            Some(_) => (), // just ignore outdated timeouts
            None => warn!(self.log(), "Got unexpected timeout: {:?}", timeout_id), // can happen during restart or teardown
        }
    }
}

impl Provide<ControlPort> for Buncher {
    fn handle(&mut self, event: ControlEvent) -> () {
        match event {
            ControlEvent::Start => {
                let timeout =
                    self.schedule_periodic(self.timeout, self.timeout, Buncher::handle_timeout);
                self.outstanding_timeout = Some(timeout);
            }
            ControlEvent::Stop | ControlEvent::Kill => {
                if let Some(timeout) = self.outstanding_timeout.take() {
                    self.cancel_timer(timeout);
                }
            }
        }
    }
}

impl Provide<Batching> for Buncher {
    fn handle(&mut self, event: Ping) -> () {
        self.current_batch.push(event);
        if self.current_batch.len() >= self.batch_size {
            self.trigger_batch();
        }
    }
}

pub fn main() {
    let system = KompactConfig::default().build().expect("system");
    let printer = system.create(BatchPrinter::new);
    let buncher = system.create(move || Buncher::new(100, Duration::from_millis(150)));
    biconnect_components::<Batching, _, _>(&buncher, &printer).expect("connection");
    let batching = buncher.on_definition(|cd| cd.batch_port.share());

    system.start(&printer);
    system.start(&buncher);

    // these should usually trigger due to full batches
    let sleep_dur = Duration::from_millis(1);
    for i in 0..500 {
        let ping = Ping(i);
        system.trigger_r(ping, &batching);
        std::thread::sleep(sleep_dur);
    }

    // these should usually trigger due to timeout
    let sleep_dur = Duration::from_millis(2);
    for i in 0..500 {
        let ping = Ping(i);
        system.trigger_r(ping, &batching);
        std::thread::sleep(sleep_dur);
    }

    system.shutdown().expect("shutdown");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_buncher() {
        main();
    }
}

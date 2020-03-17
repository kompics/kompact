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
    fn new() -> Buncher {
        Buncher {
            ctx: ComponentContext::new(),
            batch_port: ProvidedPort::new(),
            batch_size: 0,
            timeout: Duration::from_millis(1),
            current_batch: Vec::new(),
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
                let new_timeout = self.schedule_once(self.timeout, Buncher::handle_timeout);
                self.outstanding_timeout = Some(new_timeout);
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
                self.batch_size = self.ctx.config()["buncher"]["batch-size"]
                    .as_i64()
                    .expect("batch size") as usize;
                self.timeout = self.ctx.config()["buncher"]["timeout"]
                    .as_duration()
                    .expect("timeout");
                self.current_batch.reserve(self.batch_size);
                let timeout = self.schedule_once(self.timeout, Buncher::handle_timeout);
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
            if let Some(timeout) = self.outstanding_timeout.take() {
                self.cancel_timer(timeout);
            }
            let new_timeout = self.schedule_once(self.timeout, Buncher::handle_timeout);
            self.outstanding_timeout = Some(new_timeout);
        }
    }
}

pub fn main() {
    let mut conf = KompactConfig::default();
    conf.load_config_file("./application.conf")
        .load_config_str("buncher.batch-size = 50");
    let system = conf.build().expect("system");
    let printer = system.create(BatchPrinter::new);
    let buncher = system.create(Buncher::new);
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

#![allow(clippy::unused_unit)]
use kompact::prelude::*;
use kompact_examples_local::batching::*;
use std::time::Duration;

#[derive(ComponentDefinition, Actor)]
struct Buncher {
    ctx: ComponentContext<Self>,
    batch_port: ProvidedPort<Batching>,
    batch_size: usize,
    timeout: Duration,
    current_batch: Vec<Ping>,
    outstanding_timeout: Option<ScheduledTimer>,
}

impl Buncher {
    // ANCHOR: new
    fn new() -> Buncher {
        Buncher {
            ctx: ComponentContext::uninitialised(),
            batch_port: ProvidedPort::uninitialised(),
            batch_size: 0,
            timeout: Duration::from_millis(1),
            current_batch: Vec::new(),
            outstanding_timeout: None,
        }
    }

    // ANCHOR_END: new

    fn trigger_batch(&mut self) -> () {
        let mut new_batch = Vec::with_capacity(self.batch_size);
        std::mem::swap(&mut new_batch, &mut self.current_batch);
        self.batch_port.trigger(Batch(new_batch))
    }

    fn handle_timeout(&mut self, timeout_id: ScheduledTimer) -> HandlerResult {
        match self.outstanding_timeout {
            Some(ref timeout) if *timeout == timeout_id => {
                self.trigger_batch();
                let new_timeout = self.schedule_once(self.timeout, Self::handle_timeout);
                self.outstanding_timeout = Some(new_timeout);
                Handled::OK
            }
            Some(_) => Handled::OK, // just ignore outdated timeouts
            None => {
                warn!(self.log(), "Got unexpected timeout: {:?}", timeout_id);
                Handled::OK
            } // can happen during restart or teardown
        }
    }
}

impl ComponentLifecycle for Buncher {
    // ANCHOR: on_start
    fn on_start(&mut self) -> HandlerResult {
        self.batch_size = self.ctx.config()["buncher"]["batch-size"]
            .as_i64()
            .expect("batch size") as usize;
        self.timeout = self.ctx.config()["buncher"]["timeout"]
            .as_duration()
            .expect("timeout");
        self.current_batch.reserve(self.batch_size);
        let timeout = self.schedule_once(self.timeout, Buncher::handle_timeout);
        self.outstanding_timeout = Some(timeout);
        Handled::OK
    }

    // ANCHOR_END: on_start

    fn on_stop(&mut self) -> HandlerResult {
        if let Some(timeout) = self.outstanding_timeout.take() {
            self.cancel_timer(timeout);
        }
        Handled::OK
    }

    fn on_kill(&mut self) -> HandlerResult {
        self.on_stop()
    }
}

impl Provide<Batching> for Buncher {
    fn handle(&mut self, event: Ping) -> HandlerResult {
        self.current_batch.push(event);
        if self.current_batch.len() >= self.batch_size {
            self.trigger_batch();
            if let Some(timeout) = self.outstanding_timeout.take() {
                self.cancel_timer(timeout);
            }
            let new_timeout = self.schedule_once(self.timeout, Buncher::handle_timeout);
            self.outstanding_timeout = Some(new_timeout);
        }
        Handled::OK
    }
}

pub fn main() {
    // ANCHOR: system
    let mut conf = KompactConfig::default();
    // ANCHOR: config_file
    conf.load_config_file("./app_settings.toml")
        // ANCHOR_END: config_file
        .load_config_str("buncher.batch-size = 50");
    let system = conf.build().expect("system");
    // ANCHOR_END: system
    let printer = system.create(BatchPrinter::new);
    // ANCHOR: create_buncher
    let buncher = system.create(Buncher::new);
    // ANCHOR_END: create_buncher
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

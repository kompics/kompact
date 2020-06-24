use kompact::prelude::*;

#[derive(Clone, Debug)]
pub struct Ping(pub u64);

#[derive(Clone, Debug)]
pub struct Batch(pub Vec<Ping>);

pub struct Batching;
impl Port for Batching {
    type Indication = Batch;
    type Request = Ping;
}

#[derive(ComponentDefinition, Actor)]
pub struct BatchPrinter {
    ctx: ComponentContext<Self>,
    batch_port: RequiredPort<Batching>,
}
impl BatchPrinter {
    pub fn new() -> Self {
        BatchPrinter {
            ctx: ComponentContext::uninitialised(),
            batch_port: RequiredPort::uninitialised(),
        }
    }
}

ignore_control!(BatchPrinter);

impl Require<Batching> for BatchPrinter {
    fn handle(&mut self, batch: Batch) -> Handled {
        info!(self.log(), "Got a batch with {} Pings.", batch.0.len());
        Handled::Ok
    }
}

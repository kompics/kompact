use kompact::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub struct Heartbeat;
impl SerialisationId for Heartbeat {
    const SER_ID: SerId = 1234;
}

#[derive(Clone, Debug)]
pub struct Trust(pub ActorPath);

pub struct EventualLeaderDetection;
impl Port for EventualLeaderDetection {
    type Indication = Trust;
    type Request = Never;
}

#[derive(ComponentDefinition, Actor)]
pub struct TrustPrinter {
    ctx: ComponentContext<Self>,
    omega_port: RequiredPort<EventualLeaderDetection, Self>,
}
impl TrustPrinter {
    pub fn new() -> Self {
        TrustPrinter {
            ctx: ComponentContext::new(),
            omega_port: RequiredPort::new(),
        }
    }
}

ignore_control!(TrustPrinter);

impl Require<EventualLeaderDetection> for TrustPrinter {
    fn handle(&mut self, event: Trust) -> () {
        info!(self.log(), "Got leader: {}.", event.0);
    }
}

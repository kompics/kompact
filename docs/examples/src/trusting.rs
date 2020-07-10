use kompact::prelude::*;
// ANCHOR: heartbeat
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub struct Heartbeat;
impl SerialisationId for Heartbeat {
    const SER_ID: SerId = 1234;
}
// ANCHOR_END: heartbeat

// ANCHOR: port
#[derive(Clone, Debug)]
pub struct Trust(pub ActorPath);

pub struct EventualLeaderDetection;
impl Port for EventualLeaderDetection {
    type Indication = Trust;
    type Request = Never;
}
// ANCHOR_END: port

// ANCHOR: printer
#[derive(ComponentDefinition, Actor)]
pub struct TrustPrinter {
    ctx: ComponentContext<Self>,
    omega_port: RequiredPort<EventualLeaderDetection>,
}
impl TrustPrinter {
    pub fn new() -> Self {
        TrustPrinter {
            ctx: ComponentContext::uninitialised(),
            omega_port: RequiredPort::uninitialised(),
        }
    }
}

ignore_lifecycle!(TrustPrinter);

impl Require<EventualLeaderDetection> for TrustPrinter {
    fn handle(&mut self, event: Trust) -> Handled {
        info!(self.log(), "Got leader: {}.", event.0);
        Handled::Ok
    }
}
// ANCHOR_END: printer

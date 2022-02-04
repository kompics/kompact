pub mod adaptor;
pub mod core;
pub mod events;
pub mod instrumentation;
pub mod network;
pub mod result;
pub mod run;
pub mod scheduler;
pub mod stochastic;
pub mod util;

struct SimulationScenario {

}

impl Serialisable for SimulationScenario {
    fn ser_id(&self) -> SerId {
        todo!();
    }

    fn size_hint(&self) -> Option<usize> {
        todo!();
    }

    fn serialise(&self, buf: &mut dyn BufMut) -> Result<(), SerError> {
        todo!();
    }

    fn local(self: Box<Self>) -> Result<Box<dyn Any + Send>, Box<dyn Serialisable>> {
        todo!();
    } 
}
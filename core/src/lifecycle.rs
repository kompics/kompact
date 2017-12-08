
use super::*;

#[derive(Clone, Debug)]
pub enum ControlEvent {
    Start,Stop,Kill,
}

pub struct ControlPort;

impl Port for ControlPort {
    type Request = ControlEvent;
    type Indication = ();
}
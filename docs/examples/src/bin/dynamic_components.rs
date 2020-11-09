#![allow(clippy::unused_unit)]

use kompact::prelude::*;
use std::sync::Arc;
use kompact::component::AbstractComponent;
use std::fmt;
use std::io::{stdin, BufRead};
use std::error::Error;

// ANCHOR: simple_components
#[derive(ComponentDefinition)]
struct Adder {
    ctx: ComponentContext<Self>,
    offset: f32,
    set_offset: ProvidedPort<SetOffset>,
}
info_lifecycle!(Adder);

impl Actor for Adder {
    type Message = f32;

    fn receive_local(&mut self, a: Self::Message) -> Handled {
        let res = a + self.offset;
        info!(self.log(), "Adder result = {}", res);
        Handled::Ok
    }

    fn receive_network(&mut self, _msg: NetMessage) -> Handled {
        unimplemented!()
    }
}

struct SetOffset;

impl Port for SetOffset {
    type Indication = Never;
    type Request = f32;
}

impl Provide<SetOffset> for Adder {
    fn handle(&mut self, value: f32) -> Handled {
        self.offset = value;
        Handled::Ok
    }
}

impl Adder {
    pub fn new() -> Self {
        Adder {
            ctx: ComponentContext::uninitialised(),
            offset: 0f32,
            set_offset: ProvidedPort::uninitialised(),
        }
    }
}

#[derive(ComponentDefinition)]
struct Multiplier {
    ctx: ComponentContext<Self>,
    scale: f32,
    set_scale: ProvidedPort<SetScale>,
}
info_lifecycle!(Multiplier);

impl Actor for Multiplier {
    type Message = f32;

    fn receive_local(&mut self, a: Self::Message) -> Handled {
        let res = a * self.scale;
        info!(self.log(), "Multiplier result = {}", res);
        Handled::Ok
    }

    fn receive_network(&mut self, _msg: NetMessage) -> Handled {
        unimplemented!()
    }
}

struct SetScale;

impl Port for SetScale {
    type Indication = Never;
    type Request = f32;
}

impl Provide<SetScale> for Multiplier {
    fn handle(&mut self, value: f32) -> Handled {
        self.scale = value;
        Handled::Ok
    }
}

impl Multiplier {
    fn new() -> Multiplier {
        Multiplier {
            ctx: ComponentContext::uninitialised(),
            scale: 1.0,
            set_scale: ProvidedPort::uninitialised(),
        }
    }
}
// ANCHOR_END: simple_components

// ANCHOR: linear
#[derive(ComponentDefinition)]
struct Linear {
    ctx: ComponentContext<Self>,
    scale: f32,
    offset: f32,
    set_scale: ProvidedPort<SetScale>,
    set_offset: ProvidedPort<SetOffset>,
}
info_lifecycle!(Linear);

impl Actor for Linear {
    type Message = f32;

    fn receive_local(&mut self, a: Self::Message) -> Handled {
        let res = a * self.scale + self.offset;
        info!(self.log(), "Linear result = {}", res);
        Handled::Ok
    }

    fn receive_network(&mut self, _msg: NetMessage) -> Handled {
        unimplemented!()
    }
}

impl Provide<SetOffset> for Linear {
    fn handle(&mut self, value: f32) -> Handled {
        self.offset = value;
        Handled::Ok
    }
}

impl Provide<SetScale> for Linear {
    fn handle(&mut self, value: f32) -> Handled {
        self.scale = value;
        Handled::Ok
    }
}

impl Linear {
    fn new() -> Linear {
        Linear {
            ctx: ComponentContext::uninitialised(),
            scale: 1.0,
            offset: 0.0,
            set_scale: ProvidedPort::uninitialised(),
            set_offset: ProvidedPort::uninitialised(),
        }
    }
}
// ANCHOR_END: linear

// ANCHOR: storage
#[derive(ComponentDefinition)]
struct DynamicManager {
    ctx: ComponentContext<Self>,
    arithmetic_units: Vec<Arc<dyn AbstractComponent<Message=f32>>>,
    set_offsets: RequiredPort<SetOffset>,
    set_scales: RequiredPort<SetScale>,
}

ignore_indications!(SetOffset, DynamicManager);
ignore_indications!(SetScale, DynamicManager);
ignore_lifecycle!(DynamicManager);
// ANCHOR_END: storage

// ANCHOR: creation
enum ManagerMessage {
    Spawn(Box<dyn CreateErased<f32> + Send>),
    Compute(f32),
    SetScales(f32),
    SetOffsets(f32),
    KillAll,
    Quit,
}

impl fmt::Debug for ManagerMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ManagerMessage::Spawn(_) => { write!(f, "Spawn(_)") }
            ManagerMessage::Compute(val) => { write!(f, "Compute({})", *val) }
            ManagerMessage::SetScales(scale) => { write!(f, "SetScales({})", *scale) }
            ManagerMessage::SetOffsets(offset) => { write!(f, "SetOffsets({})", *offset) }
            ManagerMessage::KillAll => { write!(f, "KillAll") }
            ManagerMessage::Quit => { write!(f, "Quit") }
        }
    }
}

impl Actor for DynamicManager {
    type Message = ManagerMessage;

    fn receive_local(&mut self, msg: ManagerMessage) -> Handled {
        match msg {
            ManagerMessage::Spawn(definition) => {
                let system = self.ctx.system();
                let component = system.create_erased(definition);
                // ANCHOR_END: creation
                // ANCHOR: ports
                component.on_dyn_definition(|def| {
                    if let Some(set_scale) = def.get_provided_port::<SetScale>() {
                        biconnect_ports(set_scale, &mut self.set_scales);
                    }
                    if let Some(set_offset) = def.get_provided_port::<SetOffset>() {
                        biconnect_ports(set_offset, &mut self.set_offsets);
                    }
                });
                system.start(&component);

                self.arithmetic_units.push(component);
            }
            ManagerMessage::Compute(val) => {
                for unit in &self.arithmetic_units {
                    unit.actor_ref().tell(val);
                }
            }
            ManagerMessage::SetScales(scale) => { self.set_scales.trigger(scale) }
            ManagerMessage::SetOffsets(offset) => { self.set_offsets.trigger(offset) }
            ManagerMessage::KillAll => {
                self.kill_all();
            }
            ManagerMessage::Quit => {
                self.kill_all();
                self.ctx.system().shutdown_async();
            }
        }

        Handled::Ok
    }

    fn receive_network(&mut self, _: NetMessage) -> Handled {
        unimplemented!()
    }
}

impl DynamicManager {
    fn kill_all(&mut self) {
        let system = self.ctx.system();
        for unit in self.arithmetic_units.drain(..) {
            system.kill(unit);
        }
    }
}
// ANCHOR_END: ports

// ANCHOR: repl
fn main() {
    let system = KompactConfig::default().build().expect("system");
    let manager: Arc<Component<DynamicManager>> = system.create(|| DynamicManager {
        ctx: ComponentContext::uninitialised(),
        arithmetic_units: vec![],
        set_offsets: RequiredPort::uninitialised(),
        set_scales: RequiredPort::uninitialised(),
    });
    system.start(&manager);
    let manager_ref = manager.actor_ref();

    std::thread::spawn(move || {
        for line in stdin().lock().lines() {
            let res = (|| -> Result<(), Box<dyn Error>> {
                let line = line?;

                let message = match line.trim() {
                    "spawn adder" => { ManagerMessage::Spawn(Box::new(Adder::new())) }
                    "spawn multiplier" => { ManagerMessage::Spawn(Box::new(Multiplier::new())) }
                    "spawn linear" => { ManagerMessage::Spawn(Box::new(Linear::new())) }
                    "kill all" => { ManagerMessage::KillAll }
                    "quit" => { ManagerMessage::Quit }
                    other => {
                        if let Some(offset) = other.strip_prefix("set offset ") {
                            ManagerMessage::SetOffsets(offset.parse()?)
                        } else if let Some(scale) = other.strip_prefix("set scale ") {
                            ManagerMessage::SetScales(scale.parse()?)
                        } else if let Some(val) = other.strip_prefix("compute ") {
                            ManagerMessage::Compute(val.parse()?)
                        } else {
                            Err("unknown command!")?
                        }
                    }
                };

                manager_ref.tell(message);

                Ok(())
            })();

            if let Err(e) = res {
                println!("{}", e);
            }
        }
    });

    system.await_termination();
}
// ANCHOR_END: repl
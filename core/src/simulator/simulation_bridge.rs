use super::*;
use actors::Transport;
use arc_swap::ArcSwap;
use dispatch::lookup::ActorStore;
use futures::task::waker;

use crate::{
    dispatch::NetworkStatus,
    messaging::{DispatchData, DispatchEnvelope, EventEnvelope},
    net::{events::{DispatchEvent, self}, frames::*, network_thread::NetworkThreadBuilder, Protocol, NetworkBridgeErr},
    prelude::{NetworkConfig, SimulationNetworkConfig},
};
use crossbeam_channel::{unbounded as channel, RecvError, SendError, Sender};
use ipnet::IpNet;
use mio::{Interest, Waker};
pub use std::net::SocketAddr;
use std::{io, net::IpAddr, panic, sync::Arc, thread, time::Duration};
use uuid::Uuid;
use simulation_network::SimulationNetwork;

pub struct SimulationBridge {
    /// Network-specific configuration
    //cfg: BridgeConfig,
    /// Core logger; shared with network thread
    log: KompactLogger,
    /// Shared actor reference lookup table
    // lookup: Arc<ArcSwap<ActorStore>>,
    /// Network Thread stuff:
    // network_thread: Box<NetworkThread>,
    // ^ Can we avoid storing this by moving it into itself?
    /// Tokio Runtime
    // tokio_runtime: Option<Runtime>,
    /// Reference back to the Kompact dispatcher
    dispatcher: Option<DispatcherRef>,
    /// Socket the network actually bound on
    bound_address: Option<SocketAddr>,
    network: Arc<Mutex<SimulationNetwork>>
}

impl SimulationBridge {
    pub fn new(
        lookup: Arc<ArcSwap<ActorStore>>,
        network_log: KompactLogger,
        bridge_log: KompactLogger,
        addr: SocketAddr,
        dispatcher_ref: DispatcherRef,
        network_config: &SimulationNetworkConfig,
        network: Arc<Mutex<SimulationNetwork>>
    ) -> (Self, SocketAddr) {

        let mut s_addr = addr;

        {
            println!("REGISTERING SYSTEM");
            let mut net = network.lock().unwrap();
            s_addr.set_port(net.get_port());
            net.register_system(s_addr, lookup);
        }

        println!("THIS IS PORT: {}", s_addr.port());

        let bridge = SimulationBridge {
            log: bridge_log,
            dispatcher: Some(dispatcher_ref),
            bound_address: Some(s_addr),
            network,
        };
        (bridge, addr)
    }

    /// Returns the local address if already bound
    pub fn local_addr(&self) -> &Option<SocketAddr> {
        match self.bound_address {
            Some(addr) => println!("bound_address {}", addr.ip()),
            None => println!("NO bound_address"),
        }
        
        &self.bound_address
    }

    pub fn get_actor_store(&self) -> &Option<SocketAddr> {
        &self.bound_address
    }

    /// Sets the dispatcher reference, returning the previously stored one
    pub fn set_dispatcher(&mut self, dispatcher: DispatcherRef) -> Option<DispatcherRef> {
        std::mem::replace(&mut self.dispatcher, Some(dispatcher))
    }

    /// Forwards `serialized` to the NetworkThread and makes sure that it will wake up.
    pub(crate) fn route(
        &self,
        addr: SocketAddr,
        data: DispatchData,
        protocol: Protocol,
    ) -> Result<(), NetworkBridgeErr> {
        //println!("ROUTE IN BRIDGE");
        match protocol {
            Protocol::Tcp => {
                let _ = self.network.lock().unwrap().send(DispatchEvent::SendTcp(addr, data));
            }
            Protocol::Udp => {
                let _ = self.network.lock().unwrap().send(DispatchEvent::SendUdp(addr, data));
            }
        }
        Ok(())
    }

    pub fn connect(&self, proto: Transport, addr: SocketAddr) -> Result<(), NetworkBridgeErr> {
        todo!();
        /*match proto {
            Transport::Tcp => {
                self.network_input_queue
                    .send(events::DispatchEvent::Connect(addr))?;
                self.waker.wake()?;
                Ok(())
            }
            _other => Err(NetworkBridgeErr::Other("Bad Protocol".to_string())),
        }*/
    }
}
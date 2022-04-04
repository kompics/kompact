use arc_swap::ArcSwap;
use ipnet::{Ipv6Net, Ipv4Net, IpNet};
use iprange::{IpRange, ToNetwork};
use log::{warn, error};
use rustc_hash::FxHashSet;

use crate::{prelude::{KompactSystem, NetMessage}, net::events::DispatchEvent, messaging::DispatchData, lookup::{ActorStore, ActorLookup, LookupResult}, KompactLogger};
use std::{collections::HashMap, net::IpAddr, sync::Arc};
pub use std::net::SocketAddr;

use super::simulation_bridge::SimulationBridge;


pub struct SimulationNetwork {
    socket_lookup_map: HashMap<SocketAddr, Arc<ArcSwap<ActorStore>>>,
    log: Option<KompactLogger>,
    port_counter: u16
}

unsafe impl Send for SimulationNetwork {}
unsafe impl Sync for SimulationNetwork {}

impl SimulationNetwork{
    pub fn new() -> Self {
        SimulationNetwork{
            socket_lookup_map: HashMap::new(),
            log: None,
            port_counter: 0
        }
    }

    pub fn add_logger(&mut self, logger: KompactLogger){
        self.log = Option::Some(logger);
    }

    pub fn register_system(&mut self, mut addr: SocketAddr, actor_store: Arc<ArcSwap<ActorStore>>){
        self.socket_lookup_map.insert(addr, actor_store);
    }

    pub fn get_port(&mut self) -> u16{
        self.port_counter += 1;
        self.port_counter
    }

    pub fn send(&mut self, event: DispatchEvent) -> () {
        self.handle_dispatch_event(event);
    }

    fn handle_dispatch_event(&mut self, event: DispatchEvent) {
        match event {
            DispatchEvent::SendTcp(address, data) => {
                self.send_tcp_message(address, data);
            }
            DispatchEvent::SendUdp(address, data) => {
                self.send_udp_message(address, data);
            }
            DispatchEvent::Stop => {
                self.stop();
            }
            DispatchEvent::Kill => {
                self.kill();
            }
            DispatchEvent::Connect(addr) => {
                /*if self.block_list.socket_addr_is_blocked(&addr) {
                    return;
                }*/
                self.request_stream(addr);
            }
            DispatchEvent::ClosedAck(addr) => {
                self.handle_closed_ack(addr);
            }
            DispatchEvent::Close(addr) => {
                self.close_connection(addr);
            }
            DispatchEvent::BlockSocket(addr) => {
                self.block_socket_addr(addr);
            }
            DispatchEvent::BlockIpAddr(ip_addr) => {
                self.block_ip_addr(ip_addr);
            }
            DispatchEvent::AllowSocket(addr) => {
                self.allow_socket_addr(addr);
            }
            DispatchEvent::AllowIpAddr(ip_addr) => {
                self.allow_ip_addr(ip_addr);
            }
            DispatchEvent::BlockIpNet(net) => {
                self.block_ip_net(net);
            }
            DispatchEvent::AllowIpNet(net) => {
                self.allow_ip_net(net);
            }
        }
    }
    
    fn send_tcp_message(&mut self, address: SocketAddr, data: DispatchData){
    }

    fn send_udp_message(&mut self, address: SocketAddr, data: DispatchData){

        let local_msg = data.into_local();

        let lookup = self.socket_lookup_map.get(&address);

        match local_msg {
            Ok(envelope) => {
                match lookup {
                    Some(l) => {
                        let lease_lookup = l.load();
                        match lease_lookup.get_by_actor_path(&envelope.receiver) {
                            LookupResult::Ref(actor) => {
                                actor.enqueue(envelope);
                            },
                            LookupResult::Group(group) => {
                                match &self.log{
                                    Some(log) => group.route(envelope, &log),
                                    None => todo!(),
                                }
                            },
                            LookupResult::None => {
                                /* warn!(
                                    "{} {} {}", self.log,
                                    "Could not find actor reference for destination: {:?}, dropping message",
                                    envelope.receiver
                                );*/
                            }
                            LookupResult::Err(e) => {
                                /* error!(
                                    "{} {} {} {}", self.log,
                                    "An error occurred during local actor lookup for destination: {:?}, dropping message. The error was: {}",
                                    envelope.receiver,
                                    e
                                );*/
                            }
                        }
                    }
                    None => todo!(),
                }
            }
            Err(e) => {
                todo!();
            }
        }
    }

    fn stop(&mut self){
        todo!();
    }

    fn kill(&mut self){
        todo!();
    }

    fn request_stream(&mut self, address: SocketAddr) {
        todo!();
    }

    fn handle_closed_ack(&mut self, address: SocketAddr) {
        todo!();
    }

    fn close_connection(&mut self, address: SocketAddr) {
        todo!();
    }

    fn block_socket_addr(&mut self, address: SocketAddr) {
        todo!();
    }

    fn block_ip_addr(&mut self, address: IpAddr) {
        todo!();
    }

    fn allow_socket_addr(&mut self, address: SocketAddr) {
        todo!();
    }

    fn allow_ip_addr(&mut self, address: IpAddr) {
        todo!();
    }

    fn block_ip_net(&mut self, net: IpNet) {
        todo!();   
    }

    fn allow_ip_net(&mut self, net: IpNet) {
        todo!();
    }
}

#[derive(Default)]
pub struct BlockList {
    ipv4_set: IpRange<Ipv4Net>,
    ipv6_set: IpRange<Ipv6Net>,
    blocked_socket_addr: FxHashSet<SocketAddr>,
    allowed_socket_addr: FxHashSet<SocketAddr>,
}

impl BlockList {
    /// Returns true if the rule-set has been modified
    fn block_ip_addr(&mut self, ip_addr: IpAddr) -> bool {
        match ip_addr {
            IpAddr::V4(addr) => {
                if self.ipv4_set.contains(&addr.to_network()) {
                    return false;
                }
                self.ipv4_set.add(addr.to_network());
            }
            IpAddr::V6(addr) => {
                if self.ipv6_set.contains(&addr.to_network()) {
                    return false;
                }
                self.ipv6_set.add(addr.to_network());
            }
        }
        true
    }

    fn block_ip_net(&mut self, ip_net: IpNet) -> () {
        match ip_net {
            IpNet::V4(net) => {
                self.ipv4_set.add(net);
            }
            IpNet::V6(net) => {
                self.ipv6_set.add(net);
            }
        }
    }

    fn allow_ip_net(&mut self, ip_net: IpNet) -> () {
        match ip_net {
            IpNet::V4(net) => {
                self.ipv4_set.remove(net);
            }
            IpNet::V6(net) => {
                self.ipv6_set.remove(net);
            }
        }
    }

    /// Returns true if the rule-set has been modified
    fn allow_ip_addr(&mut self, ip_addr: &IpAddr) -> bool {
        match ip_addr {
            IpAddr::V4(addr) => {
                if self.ipv4_set.contains(&addr.to_network()) {
                    self.ipv4_set.remove(addr.to_network());
                    return true;
                }
            }
            IpAddr::V6(addr) => {
                if self.ipv6_set.contains(&addr.to_network()) {
                    self.ipv6_set.remove(addr.to_network());
                    return true;
                }
            }
        }
        false
    }

    /// Returns true if the rule-set has been modified
    fn block_socket_addr(&mut self, socket_addr: SocketAddr) -> bool {
        self.allowed_socket_addr.remove(&socket_addr)
            || self.blocked_socket_addr.insert(socket_addr)
    }

    /// Returns true if the rule-set has been modified
    fn allow_socket_addr(&mut self, socket_addr: &SocketAddr) -> bool {
        self.blocked_socket_addr.remove(socket_addr)
            || self.allowed_socket_addr.insert(*socket_addr)
    }

    /// Returns true if the IpAddr is fully blocked, i.e. it's Blocked and there's no Allowed SocketAddr with the given IP
    fn ip_addr_is_blocked(&self, ip_addr: &IpAddr) -> bool {
        if self.ip_sets_contains_ip_addr(ip_addr) {
            // The IP may be partially blocked
            !self
                .allowed_socket_addr
                .iter()
                .any(|socket_addr| socket_addr.ip() == *ip_addr)
        } else {
            // The IP isn't Blocked at all, no need to check the Socket address list
            false
        }
    }

    /// Returns true if the SocketAddr is blocked
    fn socket_addr_is_blocked(&self, socket_addr: &SocketAddr) -> bool {
        if self.allowed_socket_addr.contains(socket_addr) {
            false
        } else if self.blocked_socket_addr.contains(socket_addr) {
            true
        } else {
            self.ip_sets_contains_ip_addr(&socket_addr.ip())
        }
    }

    fn ip_sets_contains_ip_addr(&self, ip_addr: &IpAddr) -> bool {
        match ip_addr {
            IpAddr::V4(addr) => self.ipv4_set.contains(&addr.to_network()),
            IpAddr::V6(addr) => self.ipv6_set.contains(&addr.to_network()),
        }
    }
}
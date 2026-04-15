use crate::{actors::SystemPath, net::SessionId, prelude::Port};
use ipnet::IpNet;
use std::net::IpAddr;

pub mod lookup;

/// A port providing `NetworkStatus` updates to listeners.
pub struct NetworkStatusPort;
impl Port for NetworkStatusPort {
    type Indication = NetworkStatus;
    type Request = NetworkStatusRequest;
}

/// Information regarding changes to remote connectivity for a distributed system.
#[derive(Clone, Debug)]
pub enum NetworkStatus {
    /// Indicates that a connection has been established to the remote system.
    ConnectionEstablished(SystemPath, SessionId),
    /// Indicates that a connection has been lost to the remote system.
    ConnectionLost(SystemPath, SessionId),
    /// Indicates that a connection has been dropped and queued messages were discarded.
    ConnectionDropped(SystemPath),
    /// Indicates that a connection has been gracefully closed.
    ConnectionClosed(SystemPath, SessionId),
    /// Indicates that a system has been blocked.
    BlockedSystem(SystemPath),
    /// Indicates that an IP address has been blocked.
    BlockedIp(IpAddr),
    /// Indicates that an IP network has been blocked.
    BlockedIpNet(IpNet),
    /// Indicates that an IP network has been allowed after previously being blocked.
    AllowedIpNet(IpNet),
    /// Indicates that a system has been allowed after previously being blocked.
    AllowedSystem(SystemPath),
    /// Indicates that an IP address has been allowed after previously being blocked.
    AllowedIp(IpAddr),
    /// Indicates that the soft connection limit has been exceeded.
    SoftConnectionLimitExceeded,
    /// Indicates that the hard connection limit has been reached.
    HardConnectionLimitReached,
    /// Indicates that the transport layer encountered a critical failure.
    CriticalNetworkFailure,
}

/// Requests for transport-specific network status operations.
#[derive(Clone, Debug)]
pub enum NetworkStatusRequest {
    /// Request that the connection to the given system is gracefully closed.
    DisconnectSystem(SystemPath),
    /// Request that a connection is established to the given system.
    ConnectSystem(SystemPath),
    /// Request that a system path be blocked.
    BlockSystem(SystemPath),
    /// Request that an IP address be blocked.
    BlockIp(IpAddr),
    /// Request that an IP network be blocked.
    BlockIpNet(IpNet),
    /// Request that a previously blocked system be allowed again.
    AllowSystem(SystemPath),
    /// Request that a previously blocked IP address be allowed again.
    AllowIp(IpAddr),
    /// Request that a previously blocked IP network be allowed again.
    AllowIpNet(IpNet),
}

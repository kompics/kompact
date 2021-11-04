# Network Status Port

The `NetworkDispatcher` provides a `NetworkStatusPort` which any Component may use to subscribe to information about the 
Network, or make requests to the Network Layer.

## Using the Network Status Port

To subscribe to the import a component must implement `Require` for `NetworkStatusPort`. The system must be set-up with 
a `NetworkingConfig` to enable Networking. When the component is instantiated it must be explicitly connected to the 
`NetworkStatusPort`. `KompactSystem` exposes the convenience method 
`connect_network_status_port<C>(&self, required: &mut RequiredPort<NetworkStatusPort>)` to subscribe a component to the 
port, and may be used as in the example below.
```
# use kompact::prelude::*;
# use kompact::net::net_test_helpers::NetworkStatusCounter;
let mut cfg = KompactConfig::new();
cfg.system_components(DeadletterBox::new, {
    let net_config = NetworkConfig::new("127.0.0.1:0".parse().expect(""));
    net_config.build()
});
let system = cfg.build().expect("KompactSystem");
let status_counter = system.create(NetworkStatusCounter::new);
status_counter.on_definition(|c|{
  system.connect_network_status_port(&mut c.network_status_port);
})
```

## Network Status Indications

`NetworkStatus` events are the `Indications` sent by the dispatcher to the subscribed components. 
The Event is an `enum` with the following variants:

* `ConnectionEstablished(SystemPath)` Indicates that a connection has been established to the remote system
* `ConnectionLost(SystemPath)` Indicates that a connection has been lost to the remote system. The system will 
automatically try to recover the connection for a configurable amount of retries. The end of the automatic retries is 
signalled by a `ConnectionDropped` message.
* `ConnectionDropped(SystemPath)` Indicates that a connection has been dropped and no more automatic retries to 
re-establish the connection will be attempted and all queued messages have been dropped.
* `ConnectionClosed(SystemPath)` Indicates that a connection has been gracefully closed, no automatic retries will be
attempted.
*  `SoftConnectionLimitReached` Indicates that the `SoftConnectionLimit` has been reached, the `NetworkThread` will 
gracefully close the least-recently-used connection, and will continuously evict (gracefully) the LRU-connection when new 
connection attempts (incoming or outgoing) are made.
*  `HardConnectionLimitReached` Indicates that the `HardConnectionLimit` has been reached. New connection attempts will
be discarded immediately until the number of connections are lower.
*  `CriticalNetworkFailure` The `NetworkThread` has Panicked and will be restarted, any number of incoming and outgoing 
messages may have been lost.
* `BlockedSystem(SystemPath)` Indicates that a system has been blocked.
* `BlockedIp(IpAddr)` Indicates that an IpAddr has been blocked.
* `BlockedIpNet(IpNet)` Indicates that an IpAddr has been blocked.
* `AllowedSystem(SystemPath)` Indicates that a system has been unblocked after previously being blocked.
* `AllowedIp(IpAddr)` Indicates that an IpAddr has been unblocked after previously being blocked.
* `AllowedIpNet(IpNet)` Indicates that an IpNet has been unblocked after previously being blocked.

The Networking layer distinguishes between gracefully closed connections and lost connections. 
A lost connection will trigger reconnection attempts for a configurable amount of times, before it is completely dropped. 
It will also retain outgoing messages for lost connections until the connection is dropped. 

## Network Status Requests

The `NetworkDispatcher` may respond to `NetworkStatusRequest` sent by Components onto the channel. 
The event is an `enum` with the following variants:

* `DisconnectSystem(SystemPath)` Request that the connection to the given system is closed gracefully. The 
  `NetworkDispatcher`will immediately start a graceful shutdown of the channel if it is currently active.
* `ConnectSystem(SystemPath)` Request that a connection is (re-)established to the given system. Sending this message is
the only way a connection may be re-established between systems previously disconnected by a `DisconnectSystem` request.
* `BlockSystem(SystemPath)` Request that a SystemPath to be blocked from this system. An established connection
will be dropped and future attempts to establish a connection by that given SystemPath will be dropped.
* `BlockIp(IpAddr)` Request an IpAddr to be blocked. Established connections which become blocked
will be dropped and future attempts to establish a connection by that given IpAddr will be dropped.
* `BlockIpNet(IpNet)` Request an IpNet to be blocked. Established connections which become blocked
will be dropped and future attempts to establish connections to the IpNet be dropped.
* `AllowSystem(SystemPath)` This acts as an allow-list for SystemPaths. Allowed SystemPaths will always take precedence
over Blocked Ips or IpNets. This is the only way to undo a previously blocked SystemPath.
* `AllowIp(IpAddr)` Request an IpAddr to be unblocked after previously being blocked.

## Blocking and Allowing

A component that requires `NetworkStatusPort` can block a specific IP address, Network or SystemPath. 
By triggering the request `BlockIp(IpAddr)` or `BlockIpNet(IpNet)` on `NetworkStatusPort`, all connections to `IpAddr`
will be dropped and future attempts to establish a connection will also be ignored.  

The `BlockIp(IpAddr)` / `AllowIp(IpAddr)` / `BlockIpNet(IpNet)` / `AllowIpNet(IpNet)` are applied in the `NetworkThread`
in the order they are received and produce a single block-list. For example, allowing the `IpAddr` `10.0.0.1` and then
blocking the `IpNet` `10.0.0.0/24` means that the `IpAddr` will effectively become blocked. However, applying the same 
two operations in the reverse order means that the `IpAddr` will be allowed.  

The `AllowSystem(SystemPath)` / `BlockSystem(SystemPath)` are somewhat different, as the `AllowSystem()` maintains a 
separate allow-list which takes precedence over the `IpAddr`/`IpNet` block-list. So if the user first calls 
`AllowSystem(10.0.0.1:8080)` and then calls `BlockIpNet(10.0.0.0/24)` the first `AllowSystem` will take precedence and
connections to the System with the *canonical address* (listening ip:port) `10.0.0.1:8080` will remain unaffected.
To undo a previously allowed system, or to block a specific `SystemPath`, one can use `BlockSystem(SystemPath)`.

When the network thread has successfully blocked or unblocked an `IpAddr` / `IpNet` / `SystemPath`, a `BlockedIp` /
`BlockedIpNet` / `BlockedSystem` or `AllowedIp` / `AllowedIpnet` / `AllowedSystem` indication will be triggered on the 
`NetworkStatusPort`.
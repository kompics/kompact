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
* `BlockedSystem(SystemPath)` Indicates that a system has been blocked.
* `BlockedIp(IpAddr)` Indicates that an IpAddr has been blocked.
* `UnblockedSystem(SystemPath)` Indicates that a system has been unblocked after previously being blocked.
* `UnblockedIp(IpAddr)` Indicates that an IpAddr has been unblocked after previously being blocked. Unblocking an IpAddr also unblocks
all systems with that IpAddr.

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
* `BlockIp(IpAddr)` Request an IpAddr to be blocked. An established connection
will be dropped and future attempts to establish a connection by that given IpAddr will be dropped.
* `UnblockSystem(SystemPath)` Request a System to be unblocked after previously being blocked.
* `UnblockIp(IpAddr)` Request an IpAddr to be unblocked after previously being blocked.

## Blocking and Unblocking

A component that requires `NetworkStatusPort` can block a specific IP address or system. 
By triggering the request `BlockIp(IpAddr)` on `NetworkStatusPort`, all connections to `IpAddr` will be dropped and future attempts to establish a connection will also be ignored.
To only block a single system, one can use `BlockSystem(SystemPath)` which results in the same behavior but only targets the given `SystemPath` rather than all systems from a specific IP address.
If we later want to unblock an IP address or a system that was previously blocked, the corresponding `UnblockIp` or `UnblockSystem` requests can be triggered on `NetworkStatusPort`.
When the network thread has successfully blocked or unblocked an IP address or a system, a `BlockedIp`/`BlockedSystem` or `UnblockedIp`/`UnblockedSystem` indication will be triggered on the `NetworkStatusPort`.
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
* `ConnectionClosed(SystemPath)` Indicates that a connection has been gracefully closed.

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

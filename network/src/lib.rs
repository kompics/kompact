pub use kompact::{
    KompactLogger,
    component,
    config,
    default_components,
    executors,
    lookup,
    messaging,
    prelude::*,
    routing,
    runtime,
    timer,
};

pub mod actors {
    pub use kompact::prelude::{
        Actor,
        ActorPath,
        Dispatcher,
        DispatcherRef,
        DynActorRef,
        NamedPath,
        SystemPath,
        Transport,
    };
}

pub mod dispatch {
    pub use crate::dispatcher::{NetworkConfig, NetworkDispatcher};
    pub use kompact::lookup;
}

pub mod net {
    pub use crate::transport::{
        Bridge,
        ConnectionState,
        NetworkBridgeErr,
        Protocol,
        events,
        network_thread,
    };
    pub use kompact::net::{SessionId, buffers, frames};
    pub use std::net::SocketAddr;
}

mod dispatcher;
mod events;
mod queue_manager;
mod transport;

pub use crate::{
    dispatcher::{NetworkConfig, NetworkDispatcher},
    events::{NetworkStatus, NetworkStatusPort, NetworkStatusRequest},
    transport::net_test_helpers,
};

pub trait NetworkSystemExt {
    fn connect_network_status_port(&self, required: &mut RequiredPort<NetworkStatusPort>) -> ();
}

impl NetworkSystemExt for KompactSystem {
    fn connect_network_status_port(&self, required: &mut RequiredPort<NetworkStatusPort>) -> () {
        let mut connected = false;
        self.with_dispatcher_definition(|dispatcher| {
            let dispatcher = dispatcher
                .downcast_mut::<NetworkDispatcher>()
                .expect("System dispatcher is not provided by kompact-net");
            biconnect_ports(dispatcher.network_status_port(), required);
            connected = true;
        });
        debug_assert!(connected, "dispatcher callback must run exactly once");
    }
}

pub mod prelude {
    pub use crate::{NetworkConfig, NetworkDispatcher, NetworkSystemExt};
    pub use kompact::prelude::*;
}

pub use kompact::prelude::{
    NetworkConfig,
    NetworkDispatcher,
    NetworkStatus,
    NetworkStatusPort,
    NetworkStatusRequest,
};

#[cfg(feature = "distributed")]
pub use kompact::net::net_test_helpers;

pub mod prelude {
    pub use kompact::prelude::*;
    pub use crate::{
        NetworkConfig,
        NetworkDispatcher,
        NetworkStatus,
        NetworkStatusPort,
        NetworkStatusRequest,
    };
}

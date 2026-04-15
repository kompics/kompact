use uuid::Uuid;

#[allow(missing_docs)]
pub mod buffers;
pub mod frames;

pub use std::net::SocketAddr;

/// Session identifier, part of the `NetMessage` struct.
///
/// Managed by Kompact internally, and may be observed by users to detect session loss.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct SessionId(Uuid);

impl SessionId {
    /// Generates a new unique session identifier.
    pub fn new_unique() -> SessionId {
        SessionId(Uuid::new_v4())
    }

    /// Returns the internal representation for serialisation support.
    ///
    /// User code should not rely on this representation being stable.
    pub fn as_u128(&self) -> u128 {
        self.0.as_u128()
    }

    /// Reconstructs a session identifier from its serialised representation.
    ///
    /// User code should not rely on this representation being stable.
    pub fn from_u128(v: u128) -> SessionId {
        SessionId(Uuid::from_u128(v))
    }
}

//! Message framing (serialization and deserialization into and from byte buffers)

use actors::ActorPath;
use bytes::{Buf, BufMut};
use messaging::DispatchEnvelope;
use messaging::MsgEnvelope;
use messaging::ReceiveEnvelope;
use serialisation::SerError;
use serialisation::Serialisable;
use serialisation::Serialiser;
use uuid::Uuid;

#[repr(u64)]
#[derive(Debug, Copy, Clone, PartialEq, Ord, PartialOrd, Eq)]
pub enum SerIdents {
    MsgEnvelope = 0x01,
    UniqueActorPath = 0x02,
    NamedActorPath = 0x03,
    Unknown, // No ID needed
}

impl From<u64> for SerIdents {
    fn from(repr: u64) -> Self {
        match repr {
            0x01 => SerIdents::MsgEnvelope,
            0x02 => SerIdents::UniqueActorPath,
            0x03 => SerIdents::NamedActorPath,
            _ => SerIdents::Unknown,
        }
    }
}

impl Serialisable for ActorPath {
    fn id(&self) -> u64 {
        match *self {
            ActorPath::Unique(_) => SerIdents::UniqueActorPath as u64,
            ActorPath::Named(_) => SerIdents::NamedActorPath as u64,
        }
    }

    fn size_hint(&self) -> Option<usize> {
        match *self {
            ActorPath::Unique(_) => Some(16), // UuidBytes has 16 bytes
            ActorPath::Named(ref np) => {
                let len = np.path_ref().iter().fold(0, |l, s| l + s.len());
                Some(len)
            }
        }
    }

    fn serialise(&self, buf: &mut BufMut) -> Result<(), SerError> {
        unimplemented!()
    }
}

struct EnvelopeSerialiser;

impl Serialiser<DispatchEnvelope> for EnvelopeSerialiser {
    fn id(&self) -> u64 {
        SerIdents::MsgEnvelope as u64
    }

    fn serialise(&self, val: &DispatchEnvelope, buf: &mut BufMut) -> Result<(), SerError> {
        match *val {
            DispatchEnvelope::Msg {
                ref src,
                ref dst,
                ref msg,
            } => {
                msg.serialise(buf);
                println!("Serializing message to {:?}", dst);
                Ok(())
            }
            _ => unimplemented!("unsupported DispatchEnvelope for serialization"),
        }
    }
}

#[cfg(test)]
mod identity_tests {
    use super::SerIdents;
    use actors::ActorPath;

    #[test]
    fn identity() {
        let id: SerIdents = 1u64.into();
        assert_eq!(id, SerIdents::MsgEnvelope);
    }
    const PATH: &'static str = "local://127.0.0.1:0/test_actor";

    #[test]
    fn actor_path_strings() {
        use serialisation::Serialisable;
        use std::str::FromStr;
        let path = ActorPath::from_str(PATH).unwrap();
        println!("size: {:?}", path.size_hint());
    }
}

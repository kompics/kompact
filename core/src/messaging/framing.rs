//! Message framing (serialization and deserialization into and from byte buffers)

use actors::ActorPath;
use bytes::BufMut;
use serialisation::SerError;
use serialisation::Serialisable;
use std::any::Any;

#[repr(u8)]
#[derive(Debug, Copy, Clone, PartialEq, Ord, PartialOrd, Eq)]
pub enum SerIdents {
    DispatchEnvelope = 0x01,
    UniqueActorPath = 0x02,
    NamedActorPath = 0x03,
    Unknown, // No ID needed
}

impl From<u8> for SerIdents {
    fn from(repr: u8) -> Self {
        match repr {
            0x01 => SerIdents::DispatchEnvelope,
            0x02 => SerIdents::UniqueActorPath,
            0x03 => SerIdents::NamedActorPath,
            _ => SerIdents::Unknown,
        }
    }
}

impl Serialisable for ActorPath {
    fn serid(&self) -> u64 {
        match *self {
            ActorPath::Unique(_) => SerIdents::UniqueActorPath as u64,
            ActorPath::Named(_) => SerIdents::NamedActorPath as u64,
        }
    }

    fn size_hint(&self) -> Option<usize> {
        match *self {
            ActorPath::Unique(_) => Some(
                1 + // 1-byte for ActorPath type
                16, // UuidBytes has 16 bytes
            ),
            ActorPath::Named(ref _np) => {
                //                let len = np.path_ref().iter().fold(0, |l, s| l + s.len());
                //                Some(len)
                unimplemented!();
            }
        }
    }

    fn serialise(&self, buf: &mut BufMut) -> Result<(), SerError> {
        match self {
            ActorPath::Unique(up) => {
                let uuid = up.uuid_ref();
                // TODO do we need to serialize system properties as well? Probably.
                buf.put_u8(SerIdents::UniqueActorPath as u8);
                buf.put_slice(uuid.as_bytes())
            }
            ActorPath::Named(_np) => {
                unimplemented!();
//                buf.put_u8(SerIdents::NamedActorPath as u8);
                // TODO encode length and UTF-8 string of named path
            }
        }
        Ok(())
    }

    fn local(self: Box<Self>) -> Result<Box<Any + Send>, Box<Serialisable>> {
        Ok(self)
    }
}

#[cfg(test)]
mod identity_tests {
    use super::SerIdents;

    #[test]
    fn identity() {
        let id: SerIdents = 1u8.into();
        assert_eq!(id, SerIdents::DispatchEnvelope);
    }
}

//! Provides serialisation support for protocol buffers
use super::*;
use protobuf::Message;

/// Kompact serialisation marker for protobuf messages
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ProtobufSer;

impl<M: Message + Any + Debug> Serialiser<M> for ProtobufSer {
    fn ser_id(&self) -> SerId {
        serialisation_ids::PBUF
    }

    fn size_hint(&self) -> Option<usize> {
        None // no idea
    }

    fn serialise(&self, v: &M, buf: &mut dyn BufMut) -> Result<(), SerError> {
        let mut w = buf.writer();
        v.write_to_writer(&mut w).map_err(|e| SerError::ThirdParty {
            context: "protobuf".to_string(),
            source: Box::new(e),
        })
    }
}

/// Something that can be deserialised into a protobuf message of type `M`
pub struct ProtobufDeser<M: Message + Any + Debug, B: Buf> {
    /// An existing allocated message of type `M`
    pub msg: M,
    /// A buffer with data to be written into the memory at `msg`
    pub buf: B,
}

/// This implementation reuses the memory already allocated for `self.msg`
impl<M: Message + Any + Debug, B: Buf> Deserialisable<M> for ProtobufDeser<M, B> {
    fn ser_id(&self) -> SerId {
        serialisation_ids::PBUF
    }

    fn get_deserialised(self) -> Result<M, SerError> {
        let ProtobufDeser {
            msg: mut m,
            buf: mut b,
        } = self;
        let pr = {
            if b.chunk().len() < b.remaining() {
                m.merge_from_bytes(b.copy_to_bytes(b.remaining()).chunk())
            } else {
                m.merge_from_bytes(b.chunk())
            }
        };
        let r = pr.map_err(|e| SerError::ThirdParty {
            context: "protobuf".to_string(),
            source: Box::new(e),
        });
        r.map(|_| m)
    }
}

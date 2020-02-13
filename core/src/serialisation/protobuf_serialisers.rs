//! Provides serialisation support for protocol buffers
use super::*;

use protobuf::{Message, ProtobufError};

/// Kompact serialisation marker for protobuf messages
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
        v.write_to_writer(&mut w).map_err(|e| match e {
            ProtobufError::IoError(_) => {
                SerError::Unknown("Protobuf serialisation reported an IoError.".into())
            }
            ProtobufError::WireError(_) => {
                SerError::Unknown("Protobuf serialisation reported a WireError.".into())
            }
            ProtobufError::Utf8(_) => {
                SerError::Unknown("Protobuf serialisation reported an Utf8Error.".into())
            }
            ProtobufError::MessageNotInitialized { message: msg } => {
                SerError::InvalidData(format!("Protobuf serialisation reported: {}", msg).into())
            }
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
        let ProtobufDeser { msg: mut m, buf: b } = self;
        let r = m.merge_from_bytes(b.bytes()).map_err(|e| match e {
            ProtobufError::IoError(_) => {
                SerError::Unknown("Protobuf deserialisation reported an IoError.".into())
            }
            ProtobufError::WireError(_) => {
                SerError::Unknown("Protobuf deserialisation reported a WireError.".into())
            }
            ProtobufError::Utf8(_) => {
                SerError::Unknown("Protobuf deserialisation reported an Utf8Error.".into())
            }
            ProtobufError::MessageNotInitialized { message: msg } => {
                SerError::InvalidData(format!("Protobuf deserialisation reported: {}", msg).into())
            }
        });
        r.map(|_| m)
    }
}

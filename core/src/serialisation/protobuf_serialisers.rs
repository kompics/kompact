use super::*;

use protobuf::{Message, ProtobufError};

pub struct ProtobufSer;

impl<M: Message + Any + Debug> Serialiser<M> for ProtobufSer {
    fn serid(&self) -> u64 {
        serialisation_ids::PBUF
    }
    fn size_hint(&self) -> Option<usize> {
        None // no idea
    }
    fn serialise(&self, v: &M, buf: &mut BufMut) -> Result<(), SerError> {
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

pub struct ProtobufDeser<M: Message + Any + Debug, B: Buf> {
    pub msg: M,
    pub buf: B,
}

impl<M: Message + Any + Debug, B: Buf> Deserialisable<M> for ProtobufDeser<M, B> {
    fn serid(&self) -> u64 {
        serialisation_ids::PBUF
    }
    fn get_deserialised(self) -> Result<M, SerError> {
        //let (mut m, b) = self;
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

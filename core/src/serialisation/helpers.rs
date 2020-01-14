use crate::{
    actors::ActorPath,
    messaging::{NetMessage, Serialised},
    serialisation::{
        Deserialiser,
        SerError,
        SerIdBuf,
        SerIdBufMut,
        SerIdSize,
        Serialisable,
        Serialiser,
    },
};
use bytes::{Buf, Bytes, BytesMut, BufMut};
use bytes::buf::BufExt;
use crate::net::buffer::ChunkLease;
use std::sync::Arc;
use std::borrow::BorrowMut;
use crate::net::frames::{FrameHead, FrameType}; //IntoBuf
//use bytes::buf::ext::BufExt;

/// Creates a new `NetMessage` from the provided fields, allocating a new `BytesMut`
/// according to the message's size hint. The message's serialized data is stored in this buffer.
pub fn serialise_to_msg(
    src: ActorPath,
    dst: ActorPath,
    msg: Box<dyn Serialisable>,
    mut buf: ChunkLease,
) -> Result<NetMessage, SerError> {
    if let Some(size) = msg.size_hint() {
        match msg.serialise(&mut buf) {
            Ok(_) => {
                let envelope = NetMessage::with_chunk(msg.ser_id(), src, dst, buf);
                Ok(envelope)
            }
            Err(ser_err) => Err(ser_err),
        }
    } else {
        Err(SerError::Unknown("Unknown serialisation size".into()))
    }
}

/// Creates a new `Serialised` from the provided fields, allocating a new `BytesMut`
/// according to the message's size hint. The message's serialized data is stored in this buffer.
pub fn serialise_to_serialised<S>(ser: &S) -> Result<Serialised, SerError>
where
    S: Serialisable + ?Sized,
{
    if let Some(size) = ser.size_hint() {
        let mut buf = BytesMut::with_capacity(size);
        ser.serialise(&mut buf).map(|_| Serialised {
            ser_id: ser.ser_id(),
            data: buf.freeze(),
        })
    } else {
        Err(SerError::Unknown("Unknown serialisation size".into()))
    }
}
/// Creates a new `Serialised` from the provided fields, allocating a new `BytesMut`
/// according to the message's size hint. The message's serialized data is stored in this buffer.
pub fn serialiser_to_serialised<T, S>(t: &T, ser: &S) -> Result<Serialised, SerError>
where
    T: std::fmt::Debug,
    S: Serialiser<T> + ?Sized,
{
    if let Some(size) = ser.size_hint() {
        let mut buf = BytesMut::with_capacity(size);
        ser.serialise(t, &mut buf).map(|_| Serialised {
            ser_id: ser.ser_id(),
            data: buf.freeze(),
        })
    } else {
        Err(SerError::Unknown("Unknown serialisation size".into()))
    }
}

/// Serializes the provided actor paths and message into a new [BytesMut]
///
/// # Format
/// The serialized format is:
///     source          ActorPath
///         path_type   u8
///         path        `[u8; 16]` if [UniquePath], else a length-prefixed UTF-8 encoded string
///     destination     ActorPath
///         (see above for specific format)
///     ser_id          u64
///     message         raw bytes
pub fn serialise_msg(
    src: &ActorPath,
    dst: &ActorPath,
    msg: Box<dyn Serialisable>,
) -> Result<Bytes, SerError> {
    let mut size: usize = 0;
    size += src.size_hint().unwrap_or(0);
    size += dst.size_hint().unwrap_or(0);
    size += &msg.ser_id().size();
    size += msg.size_hint().unwrap_or(0);

    if size == 0 {
        return Err(SerError::InvalidData("Encoded size is zero".into()));
    }

    let mut buf = BytesMut::with_capacity(size);
    Serialisable::serialise(src, &mut buf)?;
    Serialisable::serialise(dst, &mut buf)?;
    buf.put_ser_id(msg.ser_id());
    Serialisable::serialise(msg.as_ref(), &mut buf)?;

    Ok(buf.freeze())
}
pub fn serialise_into_framed_chunk<B: Serialisable>(
    src: &ActorPath,
    dst: &ActorPath,
    mut msg: B,
    buf: &mut ChunkLease,
) -> Result<(), SerError> {
    let mut size: usize = 0;
    let content_size = msg.size_hint().unwrap_or(0);
    size += src.size_hint().unwrap_or(0);
    size += dst.size_hint().unwrap_or(0);
    size += msg.ser_id().size();
    size += content_size;
    //println!("Serializing into chunk here. Chunk bytes len {}, bytes_mut len {}, package size {}", buf.bytes().len(), buf.bytes_mut().len(), size);
    //println!("Chunk content before: {:?}", buf.bytes());
    let head = FrameHead::new(FrameType::Data, size);
    //size += FrameHead::encoded_len();
    // Frame is FrameHead+Src+Dst+SerID+Msg

    if size == 0 {
        return Err(SerError::InvalidData("Encoded size is zero".into()));
    }
    if size > buf.remaining_mut() {
        return Err(SerError::InvalidData("Encoded size is too big for the buffer".into()));
    }

    head.encode_into(buf, size as u32);
    Serialisable::serialise(src, buf)?;
    Serialisable::serialise(dst, buf)?;
    buf.put_ser_id(msg.ser_id());
    Serialisable::serialise(&msg, buf)?;
    //println!("Chunk content after: {:?}", buf.bytes());
    Ok(())
}
pub fn embed_in_msg(src: &ActorPath, dst: &ActorPath, msg: Serialised) -> Result<Bytes, SerError> {
    let mut size: usize = 0;
    size += src.size_hint().unwrap_or(0);
    size += dst.size_hint().unwrap_or(0);
    size += msg.ser_id.size();

    if size == 0 {
        return Err(SerError::InvalidData("Encoded size is zero".into()));
    }

    let mut buf = BytesMut::with_capacity(size);
    Serialisable::serialise(src, &mut buf)?;
    Serialisable::serialise(dst, &mut buf)?;
    buf.put_ser_id(msg.ser_id);

    let chained = buf.chain(msg.data);
    let bytes: Bytes = chained.into_iter().collect();

    Ok(bytes)
}

/// Extracts a [MsgEnvelope] from the provided buffer according to the format above.
pub fn deserialise_msg(mut buffer: ChunkLease) -> Result<NetMessage, SerError> {
    // if buffer.remaining() < 1 {
    //     return Err(SerError::InvalidData("Not enough bytes available".into()));
    // }
    // gonna fail below anyway
    let src = ActorPath::deserialise(&mut buffer)?;
    let dst = ActorPath::deserialise(&mut buffer)?;
    let ser_id = buffer.get_ser_id();
    //let data = buffer.to_bytes();

    let envelope = NetMessage::with_chunk(ser_id, src, dst, buffer);

    Ok(envelope)
}

// Deserializes data in the buffer according to the ActorPath structure in the [framing] module.
// pub fn deserialise_actor_path<B: Buf>(mut buf: B) -> Result<(B, ActorPath), SerError> {
//     use crate::messaging::framing::{AddressType, PathType, SystemPathHeader};
//     use std::convert::TryFrom;
//     use std::net::IpAddr;

//     // Deserialize system path
//     let fields: u8 = buf.get_u8();
//     let header = SystemPathHeader::try_from(fields)?;
//     let (mut buf, address) = match header.address_type {
//         AddressType::IPv4 => {
//             let (buf, ip) = {
//                 let ip_buf = buf.take(4);
//                 if ip_buf.remaining() < 4 {
//                     return Err(SerError::InvalidData(
//                         "Could not parse 4 bytes for IPv4 address".into(),
//                     ));
//                 }
//                 let ip = {
//                     let ip = ip_buf.bytes();
//                     IpAddr::from([ip[0], ip[1], ip[2], ip[3]])
//                 };
//                 let mut buf = ip_buf.into_inner();
//                 buf.advance(4);
//                 (buf, ip)
//             };
//             (buf, ip)
//         }
//         AddressType::IPv6 => {
//             let (buf, ip) = {
//                 let ip_buf = buf.take(16);
//                 if ip_buf.remaining() < 16 {
//                     return Err(SerError::InvalidData(
//                         "Could not parse 4 bytes for IPv4 address".into(),
//                     ));
//                 }
//                 let ip = {
//                     let ip = ip_buf.bytes();
//                     IpAddr::from([
//                         ip[0], ip[1], ip[2], ip[3], ip[5], ip[6], ip[7], ip[8], ip[9], ip[10],
//                         ip[11], ip[12], ip[13], ip[14], ip[15], ip[16],
//                     ])
//                 };
//                 let mut buf = ip_buf.into_inner();
//                 buf.advance(16);
//                 (buf, ip)
//             };
//             (buf, ip)
//         }
//         AddressType::Domain => {
//             unimplemented!();
//         }
//     };
//     let port = buf.get_u16_be();
//     let system = SystemPath::new(header.protocol, address, port);

//     let (buf, path) = match header.path_type {
//         PathType::Unique => {
//             let uuid_buf = buf.take(16);
//             let uuid = Uuid::from_slice(uuid_buf.bytes())
//                 .map_err(|_err| SerError::InvalidData("Could not parse UUID".into()))?;
//             let path = ActorPath::Unique(UniquePath::with_system(system.clone(), uuid));
//             let mut buf = uuid_buf.into_inner();
//             buf.advance(16);
//             (buf, path)
//         }
//         PathType::Named => {
//             let name_len = buf.get_u16_be() as usize;
//             let name_buf = buf.take(name_len);
//             let path = {
//                 let name = String::from_utf8_lossy(name_buf.bytes()).into_owned();
//                 let parts: Vec<&str> = name.split('/').collect();
//                 if parts.len() < 1 {
//                     return Err(SerError::InvalidData(
//                         "Could not determine name for Named path type".into(),
//                     ));
//                 }
//                 let path = parts.into_iter().map(|s| s.to_string()).collect();
//                 let path = ActorPath::Named(NamedPath::with_system(system.clone(), path));
//                 path
//             };
//             let mut buf = name_buf.into_inner();
//             buf.advance(name_len);
//             (buf, path)
//         }
//     };
//     Ok((buf, path))
// }

//! Serialisation helper functions
//!
//! These function manage buffers and size hints appropriately
//! and can be used for custom network implementations.
use crate::{
    actors::ActorPath,
    messaging::{HeapOrSer, NetData, NetMessage, Serialised},
    net::buffer::ChunkLease,
    serialisation::{Deserialiser, SerError, SerIdBuf, Serialisable, Serialiser},
};
use bytes::{buf::BufMut, Buf, BytesMut};

use crate::{
    net::{
        buffer::BufferEncoder,
        frames::{FrameHead, FrameType, FRAME_HEAD_LEN},
    },
    serialisation::ser_id::SerIdBufMut,
};

/// Creates a new [NetMessage](NetMessage) from the provided fields
///
/// It allocates a new [BytesMut](bytes::BytesMut)
/// according to the message's size hint.
/// The message's serialised data is then stored in this buffer.
pub fn serialise_to_msg(
    src: ActorPath,
    dst: ActorPath,
    msg: Box<dyn Serialisable>,
) -> Result<NetMessage, SerError> {
    if let Some(size) = msg.size_hint() {
        let mut buf = BytesMut::with_capacity(size);
        match msg.serialise(&mut buf) {
            Ok(_) => {
                let envelope = NetMessage::with_bytes(msg.ser_id(), src, dst, buf.freeze());
                Ok(envelope)
            }
            Err(ser_err) => Err(ser_err),
        }
    } else {
        Err(SerError::Unknown("Unknown serialisation size".into()))
    }
}

/// Creates a new [Serialised](Serialised) from the provided fields
///
/// It allocates a new [BytesMut](bytes::BytesMut)
/// according to the message's size hint.
/// The message's serialised data is then stored in this buffer.
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

/// Creates a new [Serialised](Serialised) from the provided fields and serialiser `ser`
///
/// It allocates a new [BytesMut](bytes::BytesMut)
/// according to the message's size hint.
/// The message's serialised data is then stored in this buffer.
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

/// Serialises the provided actor paths and message
///
/// It allocates a new [BytesMut](bytes::BytesMut)
/// according to the message's size hint.
/// The message's serialised data is then stored in this buffer.
///
/// # Format
///
/// The serialized format is:
///     source          ActorPath
///         path_type   u8
///         path        `[u8; 16]` if [unique path](ActorPath::Unique), else a length-prefixed UTF-8 encoded string
///     destination     ActorPath
///         (see above for specific format)
///     ser_id          u64
///     message         raw bytes
pub fn serialise_msg<B>(
    src: &ActorPath,
    dst: &ActorPath,
    msg: &B,
    buf: &mut BufferEncoder,
) -> Result<ChunkLease, SerError>
where
    B: Serialisable + ?Sized,
{
    // Reserve space for the header:
    buf.pad(FRAME_HEAD_LEN as usize);

    src.serialise(buf)?; // src
    dst.serialise(buf)?; // dst
    buf.put_ser_id(msg.ser_id()); // ser_id
    Serialisable::serialise(msg, buf)?; // data
    match buf.get_chunk_lease() {
        Some(mut chunk_lease) => {
            let len = chunk_lease.capacity() - FRAME_HEAD_LEN as usize; // The Data portion of the Full frame.
            chunk_lease.insert_head(FrameHead::new(FrameType::Data, len));
            assert_eq!(
                chunk_lease.capacity(),
                len + FRAME_HEAD_LEN as usize,
                "Serialized frame sizing failed"
            );
            Ok(chunk_lease)
        }
        None => Err(SerError::BufferError("Could not get chunk".to_string())),
    }
}

/// Serialise a forwarded message
///
/// Uses the same format as [serialise_msg](serialise_msg).
pub fn embed_msg(msg: NetMessage, buf: &mut BufferEncoder) -> Result<ChunkLease, SerError> {
    // Reserve space for the header:
    buf.pad(FRAME_HEAD_LEN as usize);

    msg.sender.serialise(buf)?; // src
    msg.receiver.serialise(buf)?; // dst
    let NetData { ser_id, data } = msg.data;
    buf.put_ser_id(ser_id); // ser_id
    match data {
        HeapOrSer::Boxed(b) => {
            b.serialise(buf)?;
        }
        HeapOrSer::Serialised(bytes) => {
            buf.put(bytes);
        }
        HeapOrSer::Pooled(lease) => {
            buf.put(lease.bytes());
        }
    }
    match buf.get_chunk_lease() {
        Some(mut chunk_lease) => {
            let len = chunk_lease.capacity() - FRAME_HEAD_LEN as usize; // The Data portion of the Full frame.
            chunk_lease.insert_head(FrameHead::new(FrameType::Data, len));
            assert_eq!(
                chunk_lease.capacity(),
                len + FRAME_HEAD_LEN as usize,
                "Serialized frame sizing failed"
            );
            Ok(chunk_lease)
        }
        None => Err(SerError::BufferError("Could not get chunk".to_string())),
    }
}

/// Extracts a [NetMessage](NetMessage) from the provided buffer
///
/// This expects the format from [serialise_msg](serialise_msg).
pub fn deserialise_msg<B: Buf>(mut buffer: B) -> Result<NetMessage, SerError> {
    // if buffer.remaining() < 1 {
    //     return Err(SerError::InvalidData("Not enough bytes available".into()));
    // }
    // gonna fail below anyway

    let src = ActorPath::deserialise(&mut buffer)?;
    let dst = ActorPath::deserialise(&mut buffer)?;
    let ser_id = buffer.get_ser_id();
    let data = buffer.to_bytes();

    let envelope = NetMessage::with_bytes(ser_id, src, dst, data);

    Ok(envelope)
}

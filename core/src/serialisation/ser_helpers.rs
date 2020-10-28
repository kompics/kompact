//! Serialisation helper functions
//!
//! These function manage buffers and size hints appropriately
//! and can be used for custom network implementations.
use crate::{
    actors::ActorPath,
    messaging::{HeapOrSer, NetData, NetMessage, Serialised},
    net::{
        buffers::{BufferEncoder, ChunkLease, ChunkRef},
        frames::{FrameHead, FrameType, FRAME_HEAD_LEN},
    },
    serialisation::*,
};
use bytes::{buf::BufMut, BytesMut};

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
/// It tries to reserve space for the full message in the [BufferEncoder](net::buffers::BufferEncoder)
/// according to the message's size hint. Then serialises and [frames](net::frames) the full message
/// with headers and extracts a [ChunkLease](net::buffers::ChunkLease)
///
/// # Format
///
/// The serialized and framed format is:
///     frame_head      9 bytes including indicating the type and length of the frame.
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
    // Check size hint and try to reserve space to avoid chaining
    let mut reserve_size = 0;
    if let Some(hint) = msg.size_hint() {
        reserve_size += hint;
    }
    if let Some(hint) = src.size_hint() {
        reserve_size += hint;
    }
    if let Some(hint) = dst.size_hint() {
        reserve_size += hint;
    }
    buf.try_reserve(reserve_size + FRAME_HEAD_LEN as usize);

    // Make space for the header:
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
                "Serialized frame length faulty {:#?}",
                chunk_lease
            );
            Ok(chunk_lease)
        }
        None => Err(SerError::BufferError("Could not get chunk".to_string())),
    }
}

/// Serialises the msg into `buf` and returns a [ChunkRef](net::buffers::ChunkRef) of the serialised
/// data to be used in [tell_preserialised()](actors::ActorPath#method.tell_preserialised)
pub fn preserialise_msg<B>(msg: &B, buf: &mut BufferEncoder) -> Result<ChunkRef, SerError>
where
    B: Serialisable + ?Sized,
{
    if let Some(hint) = msg.size_hint() {
        buf.try_reserve(hint);
    }
    buf.put_ser_id(msg.ser_id()); // ser_id
    Serialisable::serialise(msg, buf)?; // data
    if let Some(chunk_lease) = buf.get_chunk_lease() {
        Ok(chunk_lease.into_chunk_ref())
    } else {
        Err(SerError::BufferError("Could not get chunk".to_string()))
    }
}

/// Serialises message- and frame-headers and appends the `content`
/// [ChunkRef](net::buffers::ChunkRef) returning a complete network-message as a `ChunkRef`.
pub fn serialise_msg_with_preserialised(
    src: &ActorPath,
    dst: &ActorPath,
    content: ChunkRef,
    buf: &mut BufferEncoder,
) -> Result<ChunkRef, SerError> {
    // Reserve space for the header:
    buf.pad(FRAME_HEAD_LEN as usize);
    src.serialise(buf)?; // src
    dst.serialise(buf)?; // dst
    if let Some(mut header) = buf.get_chunk_lease() {
        let len = header.capacity() + content.capacity() - FRAME_HEAD_LEN as usize;
        header.insert_head(FrameHead::new(FrameType::Data, len));
        Ok(header.into_chunk_ref_with_tail(content))
    } else {
        Err(SerError::BufferError("Could not get chunk".to_string()))
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
        HeapOrSer::ChunkLease(chunk_lease) => {
            buf.put(chunk_lease);
        }
        HeapOrSer::ChunkRef(chunk_ref) => {
            buf.put(chunk_ref);
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
pub fn deserialise_chunk_lease(mut buffer: ChunkLease) -> Result<NetMessage, SerError> {
    // if buffer.remaining() < 1 {
    //     return Err(SerError::InvalidData("Not enough bytes available".into()));
    // }
    // gonna fail below anyway

    let src = ActorPath::deserialise(&mut buffer)?;
    let dst = ActorPath::deserialise(&mut buffer)?;
    let ser_id = buffer.get_ser_id();

    let envelope = NetMessage::with_chunk_ref(ser_id, src, dst, buffer.into_chunk_ref());

    Ok(envelope)
}

/// Extracts a [NetMessage](NetMessage) from the provided buffer
///
/// This expects the format from [serialise_msg](serialise_msg).
pub fn deserialise_chunk_ref(mut buffer: ChunkRef) -> Result<NetMessage, SerError> {
    // if buffer.remaining() < 1 {
    //     return Err(SerError::InvalidData("Not enough bytes available".into()));
    // }
    // gonna fail below anyway

    let src = ActorPath::deserialise(&mut buffer)?;
    let dst = ActorPath::deserialise(&mut buffer)?;
    let ser_id = buffer.get_ser_id();

    let envelope = NetMessage::with_chunk_ref(ser_id, src, dst, buffer);

    Ok(envelope)
}

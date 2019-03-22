use crate::actors::ActorPath;
use crate::actors::NamedPath;
use crate::actors::SystemPath;
use crate::actors::UniquePath;
use crate::messaging::MsgEnvelope;
use crate::messaging::ReceiveEnvelope;
use crate::serialisation::SerError;
use crate::serialisation::Serialisable;
use bytes::{Buf, Bytes, BytesMut};
use uuid::Uuid;

/// Creates a new `ReceiveEnvelope` from the provided fields, allocating a new `BytesMut`
/// according to the message's size hint. The message's serialized data is stored in this buffer.
pub fn serialise_to_recv_envelope(
    src: ActorPath,
    dst: ActorPath,
    msg: Box<Serialisable>,
) -> Result<MsgEnvelope, SerError> {
    if let Some(size) = msg.size_hint() {
        let mut buf = BytesMut::with_capacity(size);
        match msg.serialise(&mut buf) {
            Ok(_) => {
                let envelope = MsgEnvelope::Receive(ReceiveEnvelope::Msg {
                    src,
                    dst,
                    ser_id: msg.serid(),
                    data: buf.freeze(),
                });
                Ok(envelope)
            }
            Err(ser_err) => Err(ser_err),
        }
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
    msg: Box<Serialisable>,
) -> Result<Bytes, SerError> {
    use bytes::BytesMut;
    let mut size: usize = 0;
    size += src.size_hint().unwrap_or(0);
    size += dst.size_hint().unwrap_or(0);
    size += Serialisable::size_hint(&msg.serid()).unwrap_or(0);
    size += msg.size_hint().unwrap_or(0);

    if size == 0 {
        return Err(SerError::InvalidData("Encoded size is zero".into()));
    }

    let mut buf = BytesMut::with_capacity(size);
    Serialisable::serialise(src, &mut buf)?;
    Serialisable::serialise(dst, &mut buf)?;
    Serialisable::serialise(&msg.serid(), &mut buf)?;
    Serialisable::serialise(msg.as_ref(), &mut buf)?;

    Ok(buf.freeze())
}

/// Extracts a [MsgEnvelope] from the provided buffer according to the format above.
pub fn deserialise_msg<B: Buf>(mut buffer: B) -> Result<ReceiveEnvelope, SerError> {
    if buffer.remaining() < 1 {
        return Err(SerError::InvalidData("Not enough bytes available".into()));
    }

    let (mut buffer, src) = deserialise_actor_path(&mut buffer)?;
    let (buffer, dst) = deserialise_actor_path(&mut buffer)?;
    let ser_id: u64 = buffer.get_u64_be();
    let data = buffer.bytes().into();

    let envelope = ReceiveEnvelope::Msg {
        src,
        dst,
        ser_id,
        data,
    };

    Ok(envelope)
}

/// Deserializes data in the buffer according to the ActorPath strucutre in the [framing] module.
fn deserialise_actor_path<B: Buf>(mut buf: B) -> Result<(B, ActorPath), SerError> {
    use crate::messaging::framing::{AddressType, PathType, SystemPathHeader};
    use std::convert::TryFrom;
    use std::net::IpAddr;

    // Deserialize system path
    let fields: u8 = buf.get_u8();
    let header = SystemPathHeader::try_from(fields)?;
    let (mut buf, address) = match header.address_type {
        AddressType::IPv4 => {
            let (buf, ip) = {
                let ip_buf = buf.take(4);
                if ip_buf.remaining() < 4 {
                    return Err(SerError::InvalidData(
                        "Could not parse 4 bytes for IPv4 address".into(),
                    ));
                }
                let ip = {
                    let ip = ip_buf.bytes();
                    IpAddr::from([ip[0], ip[1], ip[2], ip[3]])
                };
                let mut buf = ip_buf.into_inner();
                buf.advance(4);
                (buf, ip)
            };
            (buf, ip)
        }
        AddressType::IPv6 => {
            let (buf, ip) = {
                let ip_buf = buf.take(16);
                if ip_buf.remaining() < 16 {
                    return Err(SerError::InvalidData(
                        "Could not parse 4 bytes for IPv4 address".into(),
                    ));
                }
                let ip = {
                    let ip = ip_buf.bytes();
                    IpAddr::from([
                        ip[0], ip[1], ip[2], ip[3], ip[5], ip[6], ip[7], ip[8], ip[9], ip[10],
                        ip[11], ip[12], ip[13], ip[14], ip[15], ip[16],
                    ])
                };
                let mut buf = ip_buf.into_inner();
                buf.advance(16);
                (buf, ip)
            };
            (buf, ip)
        }
        AddressType::Domain => {
            unimplemented!();
        }
    };
    let port = buf.get_u16_be();
    let system = SystemPath::new(header.protocol, address, port);

    let (buf, path) = match header.path_type {
        PathType::Unique => {
            let uuid_buf = buf.take(16);
            let uuid = Uuid::from_slice(uuid_buf.bytes())
                .map_err(|_err| SerError::InvalidData("Could not parse UUID".into()))?;
            let path = ActorPath::Unique(UniquePath::with_system(system.clone(), uuid));
            let mut buf = uuid_buf.into_inner();
            buf.advance(16);
            (buf, path)
        }
        PathType::Named => {
            let name_len = buf.get_u16_be() as usize;
            let name_buf = buf.take(name_len);
            let path = {
                let name = String::from_utf8_lossy(name_buf.bytes()).into_owned();
                let parts: Vec<&str> = name.split('/').collect();
                if parts.len() < 1 {
                    return Err(SerError::InvalidData(
                        "Could not determine name for Named path type".into(),
                    ));
                }
                let path = parts.into_iter().map(|s| s.to_string()).collect();
                let path = ActorPath::Named(NamedPath::with_system(system.clone(), path));
                path
            };
            let mut buf = name_buf.into_inner();
            buf.advance(name_len);
            (buf, path)
        }
    };
    Ok((buf, path))
}

#[cfg(test)]
mod helper_tests {
    use super::Serialisable;
    use crate::actors::ActorPath;
    use crate::actors::SystemField;
    use crate::actors::SystemPath;
    use crate::actors::Transport;
    use crate::actors::UniquePath;
    use crate::serialisation::helpers::deserialise_actor_path;
    use bytes::BytesMut;
    use bytes::{BufMut, IntoBuf};
    use std::net::IpAddr;
    use uuid::Uuid;

    #[test]
    fn actor_path_ser_deser_equivalence() {
        let expected_transport: Transport = Transport::TCP;
        let expected_addr: IpAddr = "12.0.0.1".parse().unwrap();
        let unique_id: Uuid = Uuid::new_v4();
        let port: u16 = 1234;

        let path = ActorPath::Unique(UniquePath::new(
            expected_transport,
            expected_addr,
            port,
            unique_id,
        ));

        let size = Serialisable::size_hint(&path).unwrap();
        let mut buf = BytesMut::with_capacity(size);
        let res = Serialisable::serialise(&path, &mut buf);
        assert!(res.is_ok(), "UUID ActorPath Serialization should succeed");

        // Deserialize
        let buf = buf.into_buf();
        let res = deserialise_actor_path(buf);
        assert!(res.is_ok(), "UUID ActorPath Deserialization should succeed");
        let (remainder, actor) = res.unwrap();
        assert_eq!(remainder.remaining_mut(), 0);
        let deser_sys: &SystemPath = SystemField::system(&actor);
        assert_eq!(deser_sys.address(), &expected_addr);
        match actor {
            ActorPath::Unique(ref up) => {
                assert_eq!(up.uuid_ref(), &unique_id);
            }
            ActorPath::Named(_) => panic!("expected Unique path, got Named path"),
        }
    }
}

//! Message framing (serialization and deserialization into and from byte buffers)

use crate::{
    actors::{ActorPath, NamedPath, SystemPath, Transport, UniquePath},
    serialisation::{Deserialiser, SerError, Serialisable},
};
use bitfields::BitField;
use bytes::{Buf, BufMut};
use std::{any::Any, convert::TryFrom, net::IpAddr};
use uuid::Uuid;

#[repr(u8)]
#[derive(Debug, Copy, Clone, PartialEq, Ord, PartialOrd, Eq)]
pub enum SerIdents {
    DispatchEnvelope = 0x01,
    UniqueActorPath = 0x02,
    NamedActorPath = 0x03,
    SystemPathHeader = 0x04,
    SystemPath = 0x05,
    Unknown, // No ID needed
}

impl From<u8> for SerIdents {
    fn from(repr: u8) -> Self {
        match repr {
            0x01 => SerIdents::DispatchEnvelope,
            0x02 => SerIdents::UniqueActorPath,
            0x03 => SerIdents::NamedActorPath,
            0x04 => SerIdents::SystemPathHeader,
            0x05 => SerIdents::SystemPath,
            _ => SerIdents::Unknown,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(u8)]
pub enum AddressType {
    IPv4 = 0,
    IPv6 = 1,
    Domain = 2,
}

#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(u8)]
pub enum PathType {
    Unique = 0,
    Named = 1,
}

impl BitField for AddressType {
    const POS: usize = 2;
    const WIDTH: usize = 2;
}

impl BitField for PathType {
    const POS: usize = 7;
    const WIDTH: usize = 1;
}

impl BitField for Transport {
    const POS: usize = 0;
    const WIDTH: usize = 5;
}

impl Into<u8> for AddressType {
    fn into(self) -> u8 {
        self as u8
    }
}

impl Into<u8> for PathType {
    fn into(self) -> u8 {
        self as u8
    }
}

impl Into<u8> for Transport {
    fn into(self) -> u8 {
        self as u8
    }
}

impl TryFrom<u8> for AddressType {
    type Error = SerError;

    fn try_from(x: u8) -> Result<Self, Self::Error> {
        match x {
            x if x == AddressType::IPv4 as u8 => Ok(AddressType::IPv4),
            x if x == AddressType::IPv6 as u8 => Ok(AddressType::IPv6),
            _ => Err(SerError::InvalidType("Unsupported AddressType".into())),
        }
    }
}

impl TryFrom<u8> for PathType {
    type Error = SerError;

    fn try_from(x: u8) -> Result<Self, Self::Error> {
        match x {
            x if x == PathType::Unique as u8 => Ok(PathType::Unique),
            x if x == PathType::Named as u8 => Ok(PathType::Named),
            _ => Err(SerError::InvalidType("Unsupported PathType".into())),
        }
    }
}

impl TryFrom<u8> for Transport {
    type Error = SerError;

    fn try_from(x: u8) -> Result<Self, Self::Error> {
        match x {
            x if x == Transport::LOCAL as u8 => Ok(Transport::LOCAL),
            x if x == Transport::UDP as u8 => Ok(Transport::UDP),
            x if x == Transport::TCP as u8 => Ok(Transport::TCP),
            _ => Err(SerError::InvalidType(
                "Unsupported transport protocol".into(),
            )),
        }
    }
}

impl<'a> From<&'a IpAddr> for AddressType {
    fn from(addr: &'a IpAddr) -> Self {
        match addr {
            IpAddr::V4(_) => AddressType::IPv4,
            IpAddr::V6(_) => AddressType::IPv6,
        }
    }
}

#[derive(Debug)]
pub struct SystemPathHeader {
    storage: [u8; 1],
    pub(crate) path_type: PathType,
    pub(crate) protocol: Transport,
    pub(crate) address_type: AddressType,
}

impl SystemPathHeader {
    pub fn from_path(sys: &ActorPath) -> Self {
        use crate::actors::SystemField;
        use bitfields::BitFieldExt;

        let path_type = match sys {
            ActorPath::Unique(_) => PathType::Unique,
            ActorPath::Named(_) => PathType::Named,
        };
        let address_type: AddressType = sys.address().into();

        let mut storage = [0u8];
        storage
            .store(path_type)
            .expect("path_type could not be stored");
        storage
            .store(sys.protocol())
            .expect("protocol could not be stored");
        storage
            .store(address_type)
            .expect("address could not be stored");

        SystemPathHeader {
            storage,
            path_type,
            protocol: sys.protocol(),
            address_type: sys.address().into(),
        }
    }
}

impl TryFrom<u8> for SystemPathHeader {
    type Error = SerError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        use bitfields::BitFieldExt;

        let storage = [value];
        let path_type = storage.get_as::<PathType>().unwrap();
        let protocol = storage.get_as::<Transport>().unwrap();
        let address_type = storage.get_as::<AddressType>().unwrap();

        let header = SystemPathHeader {
            storage,
            path_type,
            protocol,
            address_type,
        };
        Ok(header)
    }
}

impl Serialisable for SystemPathHeader {
    fn serid(&self) -> u64 {
        SerIdents::SystemPathHeader as u64
    }

    fn size_hint(&self) -> Option<usize> {
        // 1 byte containing
        //      1 bit for path type
        //      5 bits for protocol
        //      2 bits for address type
        Some(1)
    }

    fn serialise(&self, buf: &mut dyn BufMut) -> Result<(), SerError> {
        // Grab the underlying Big-Endian 8-bit storage from named fields
        buf.put_u8(self.storage[0]);
        Ok(())
    }

    fn local(self: Box<Self>) -> Result<Box<dyn Any + Send>, Box<dyn Serialisable>> {
        unimplemented!()
    }
}

// # Actor Path Serialization
// An actor path is either Unique or Named and contains a [SystemPath].
//
// # Unique Paths
//  ```
// +---------------------------+
// | System path (*)         ...
// +---------------+-----------+
// |      UUID (16 bytes)      |
// +---------------------------+
// ```
// # Named  Paths
// ```
// +---------------------------+
// | System path (*)         ...
// +---------------+-----------+-------------------------------+
// |      Named path (2 bytes prefix + variable length)      ...
// +-----------------------------------------------------------+
// ```
//
// # System Paths
// ```
// +-------------------+-------------------+-----------------------+
// | Path type (1 bit) | Protocol (5 bits) | Address Type (2 bits) |
// +-------------------+-------------------+-----------------------+----------------+
// |                   Address (4/16/ * bytes)                  ...| Port (2 bytes) |
// +---------------------------------------------------------------+----------------+
// ```
impl Serialisable for SystemPath {
    fn serid(&self) -> u64 {
        SerIdents::SystemPath as u64
    }

    fn size_hint(&self) -> Option<usize> {
        let mut size: usize = 0;
        size += 1; // header
        size += match self.address() {
            IpAddr::V4(_) => 4,  // IPv4 uses 4 bytes
            IpAddr::V6(_) => 16, // IPv4 uses 16 bytes
        };
        size += 2; // port # (0-65_535)
        Some(size)
    }

    fn serialise(&self, buf: &mut dyn BufMut) -> Result<(), SerError> {
        if buf.remaining_mut() < self.size_hint().unwrap_or(0) {
            return Err(SerError::InvalidData(format!(
                "Provided buffer has {:?} bytes, SystemPath requries {:?}.",
                buf.remaining_mut(),
                self.size_hint()
            )));
        }

        Ok(())
    }

    fn local(self: Box<Self>) -> Result<Box<dyn Any + Send>, Box<dyn Serialisable>> {
        unimplemented!()
    }
}

impl Serialisable for ActorPath {
    fn serid(&self) -> u64 {
        match self {
            ActorPath::Unique(_) => SerIdents::UniqueActorPath as u64,
            ActorPath::Named(_) => SerIdents::NamedActorPath as u64,
        }
    }

    // Returns the total size for this actor path, including system path information.
    // Returns `None` if `ActorPath::Named` and the name overflows the designated 2 bytes.
    fn size_hint(&self) -> Option<usize> {
        use crate::actors::SystemField;

        let mut size: usize = 0;
        size += 1; // System path header byte
        size += match self.address() {
            IpAddr::V4(_) => 4, // IPv4 uses 4 bytes
            IpAddr::V6(_) => 16, // IPv4 uses 16 bytes
                                 // TODO support Domain addressable type
        };
        size += 2; // port # (0-65_535)

        size += match *self {
            ActorPath::Unique(_) => {
                // UUIDs are 16 bytes long (see [UuidBytes])
                16
            }
            ActorPath::Named(ref np) => {
                // Named paths are length-prefixed (2 bytes)
                // followed by variable-length name
                let path_len: u16 = 2;
                let name_len = np.path_ref().join("/").len();
                let name_len = u16::try_from(name_len).ok()?;
                path_len.checked_add(name_len)? as usize
            }
        };
        Some(size)
    }

    /// Serializes a Unique or Named actor path.
    ///
    /// See `deserialise_msg` in kompact::serialisation for matching deserialisation.
    fn serialise(&self, buf: &mut dyn BufMut) -> Result<(), SerError> {
        use crate::actors::SystemField;

        // System Path
        let header = SystemPathHeader::from_path(&self);
        Serialisable::serialise(&header, buf)?;
        match *self.address() {
            IpAddr::V4(ref ip) => buf.put_slice(&ip.octets()),
            IpAddr::V6(ref ip) => buf.put_slice(&ip.octets()),
            // TODO support named Domain
        }
        buf.put_u16_be(self.port());

        // Actor Path
        match self {
            ActorPath::Unique(up) => {
                let uuid = up.uuid_ref();
                buf.put_slice(uuid.as_bytes())
            }
            ActorPath::Named(np) => {
                // TODO avoid this, as it constructs the join AGAIN (in fact avoid the join in the size_hint, too)
                let _path_len: u16 = self.size_hint().ok_or(SerError::InvalidData(
                    "Named path overflows designated 2 bytes.".into(),
                ))? as u16;
                let path = np.path_ref().join("/");
                let data = path.as_bytes();
                buf.put_u16_be(data.len() as u16);
                buf.put_slice(data);
            }
        }
        Ok(())
    }

    fn local(self: Box<Self>) -> Result<Box<dyn Any + Send>, Box<dyn Serialisable>> {
        Ok(self)
    }
}
impl Deserialiser<ActorPath> for ActorPath {
    fn deserialise(buf: &mut dyn Buf) -> Result<ActorPath, SerError> {
        // Deserialize system path
        let fields: u8 = buf.get_u8();
        let header = SystemPathHeader::try_from(fields)?;
        let address: IpAddr = match header.address_type {
            AddressType::IPv4 => {
                if buf.remaining() < 4 {
                    return Err(SerError::InvalidData(
                        "Could not parse 4 bytes for IPv4 address".into(),
                    ));
                } else {
                    let mut ip_bytes = [0u8; 4];
                    buf.copy_to_slice(&mut ip_bytes);
                    IpAddr::from(ip_bytes)
                }
            }
            AddressType::IPv6 => {
                if buf.remaining() < 16 {
                    return Err(SerError::InvalidData(
                        "Could not parse 16 bytes for IPv6 address".into(),
                    ));
                } else {
                    let mut ip_bytes = [0u8; 16];
                    buf.copy_to_slice(&mut ip_bytes);
                    IpAddr::from(ip_bytes)
                }
            }
            AddressType::Domain => {
                unimplemented!();
            }
        };
        let port = buf.get_u16_be();
        let system = SystemPath::new(header.protocol, address, port);

        let path = match header.path_type {
            PathType::Unique => {
                if buf.remaining() < 16 {
                    return Err(SerError::InvalidData(
                        "Could not get 16 bytes for UUID".into(),
                    ));
                } else {
                    let mut uuid_bytes = [0u8; 16];
                    buf.copy_to_slice(&mut uuid_bytes);
                    let uuid = Uuid::from_bytes(uuid_bytes);
                    //    .map_err(|_err| )?;
                    ActorPath::Unique(UniquePath::with_system(system, uuid))
                }
            }
            PathType::Named => {
                let name_len = buf.get_u16_be() as usize;
                if buf.remaining() < name_len {
                    return Err(SerError::InvalidData(format!(
                        "Could not get {} bytes for path name",
                        name_len
                    )));
                } else {
                    let mut name_bytes = vec![0u8; name_len];
                    buf.copy_to_slice(&mut name_bytes);
                    let name = unsafe {
                        // since we serialised it ourselves, this should be fine
                        String::from_utf8_unchecked(name_bytes)
                    };
                    let parts: Vec<&str> = name.split('/').collect();
                    if parts.len() < 1 {
                        return Err(SerError::InvalidData(
                            "Could not determine name for Named path type".into(),
                        ));
                    } else {
                        let path = parts.into_iter().map(|s| s.to_string()).collect();
                        ActorPath::Named(NamedPath::with_system(system.clone(), path))
                    }
                }
            }
        };
        Ok(path)
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

#[cfg(test)]
mod serialisation_tests {
    use super::*;
    use crate::actors::SystemField;
    use bytes::{BytesMut, IntoBuf};

    #[test]
    fn system_path_header() {
        use super::{PathType, SystemPathHeader};
        use crate::{
            actors::{ActorPath, NamedPath, SystemPath, Transport},
            messaging::framing::AddressType,
        };
        use bytes::{Buf, IntoBuf};
        use std::convert::TryFrom;

        let system_path = SystemPath::new(Transport::TCP, "127.0.0.1".parse().unwrap(), 8080_u16);
        let actor_path = ActorPath::Named(NamedPath::with_system(
            system_path,
            vec!["actor-name".into()],
        ));
        let header = SystemPathHeader::from_path(&actor_path);
        let mut buf = BytesMut::with_capacity(1);
        Serialisable::serialise(&header, &mut buf).unwrap();

        let _path_type = PathType::Named as u8;
        let _protocol = Transport::TCP as u8;
        let _address_type = AddressType::IPv4 as u8;

        let mut buf = buf.freeze().into_buf();
        let actual = buf.get_u8();
        let deserialised = SystemPathHeader::try_from(actual);

        assert!(deserialised.is_ok(), "Error parsing the serialised header");
        let deserialised = deserialised.unwrap();
        assert_eq!(deserialised.path_type, PathType::Named);
        assert_eq!(deserialised.protocol, Transport::TCP);
        assert_eq!(deserialised.address_type, AddressType::IPv4);
        assert_eq!(
            buf.remaining(),
            0,
            "There are remaining bytes in the buffer after serialisation."
        );
    }

    #[test]
    fn actor_path_ser_deser_equivalence() {
        let expected_transport: Transport = Transport::TCP;
        let expected_addr: IpAddr = "12.0.0.1".parse().unwrap();
        let unique_id: Uuid = Uuid::new_v4();
        let port: u16 = 1234;

        let unique_path = ActorPath::Unique(UniquePath::new(
            expected_transport,
            expected_addr,
            port,
            unique_id,
        ));

        let name: Vec<String> = vec!["test", "me", "please"]
            .into_iter()
            .map(|s| s.to_string())
            .collect();
        let named_path = ActorPath::Named(NamedPath::new(
            expected_transport,
            expected_addr,
            port,
            name.clone(),
        ));

        // unique paths
        {
            let size = Serialisable::size_hint(&unique_path).unwrap();
            let mut buf = BytesMut::with_capacity(size);
            Serialisable::serialise(&unique_path, &mut buf)
                .expect("UUID ActorPath Serialisation should succeed");

            // Deserialise
            let mut buf = buf.into_buf();
            let deser_path = ActorPath::deserialise(&mut buf)
                .expect("UUID ActorPath Deserialisation should succeed");
            assert_eq!(buf.remaining_mut(), 0);
            let deser_sys: &SystemPath = SystemField::system(&deser_path);
            assert_eq!(deser_sys.address(), &expected_addr);
            match deser_path {
                ActorPath::Unique(ref up) => {
                    assert_eq!(up.uuid_ref(), &unique_id);
                }
                ActorPath::Named(_) => panic!("expected Unique path, got Named path"),
            }
        }

        // named paths
        {
            let size = Serialisable::size_hint(&named_path).unwrap();
            let mut buf = BytesMut::with_capacity(size);
            Serialisable::serialise(&named_path, &mut buf)
                .expect("Named ActorPath Serialisation should succeed");

            // Deserialise
            let mut buf = buf.into_buf();
            let deser_path = ActorPath::deserialise(&mut buf)
                .expect("Named ActorPath Deserialisation should succeed");
            assert_eq!(buf.remaining_mut(), 0);
            let deser_sys: &SystemPath = SystemField::system(&deser_path);
            assert_eq!(deser_sys.address(), &expected_addr);
            match deser_path {
                ActorPath::Unique(_) => panic!("expected Named path, got Unique path"),
                ActorPath::Named(ref np) => {
                    assert_eq!(np.path_ref(), &name);
                }
            }
        }
    }
}

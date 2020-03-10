//! Message framing (serialization and deserialization into and from byte buffers)

use crate::{
    actors::{ActorPath, NamedPath, SystemField, SystemPath, Transport, UniquePath},
    serialisation::{serialisation_ids, Deserialiser, SerError, SerId, Serialisable},
};
use bitfields::BitField;
use bytes::{Buf, BufMut};
use std::{any::Any, convert::TryFrom, net::IpAddr};
use uuid::Uuid;

/// The type of address used
#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(u8)]
pub enum AddressType {
    /// An IPv4 address
    IPv4 = 0,
    /// An IPv6 address
    IPv6 = 1,
    /// A domain name
    Domain = 2,
}

/// The type of path used
#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(u8)]
pub enum PathType {
    /// A [unique path](ActorPath::Unique)
    Unique = 0,
    /// A [named path](ActorPath::Named)
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

/// The header for a [system path](SystemPath)
#[derive(Debug)]
pub struct SystemPathHeader {
    storage: [u8; 1],
    pub(crate) path_type: PathType,
    pub(crate) protocol: Transport,
    pub(crate) address_type: AddressType,
}

impl SystemPathHeader {
    /// Create header from an actor path
    pub fn from_path(sys: &ActorPath) -> Self {
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

    /// Create header from a system path
    pub fn from_system(sys: &SystemPath) -> Self {
        use bitfields::BitFieldExt;

        let path_type = PathType::Unique; // doesn't matter, will be ignored anyway
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

    /// Put this header's data into the give buffer
    pub fn put_into(&self, buf: &mut dyn BufMut) {
        buf.put_u8(self.storage[0])
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

// don't think this is needed
// impl Serialisable for SystemPathHeader {
//     fn serid(&self) -> u64 {
//         SerIdents::SystemPathHeader as u64
//     }

//     fn size_hint(&self) -> Option<usize> {
//         // 1 byte containing
//         //      1 bit for path type
//         //      5 bits for protocol
//         //      2 bits for address type
//         Some(1)
//     }

//     fn serialise(&self, buf: &mut dyn BufMut) -> Result<(), SerError> {
//         // Grab the underlying Big-Endian 8-bit storage from named fields
//         buf.put_u8(self.storage[0]);
//         Ok(())
//     }

//     fn local(self: Box<Self>) -> Result<Box<dyn Any + Send>, Box<dyn Serialisable>> {
//         unimplemented!()
//     }
// }

/// # Actor Path Serialization
/// An actor path is either Unique or Named and contains a [SystemPath].
/// The SystemPath's header disambiguates the type (Path type).
///
/// # Unique Actor Paths
///  ```text
/// +---------------------------+
/// | System path (*)         ...
/// +---------------+-----------+
/// |      UUID (16 bytes)      |
/// +---------------------------+
/// ```
/// # Named Actor Paths
/// ```text
/// +---------------------------+
/// | System path (*)         ...
/// +---------------+-----------+-------------------------------+
/// |      Named path (2 bytes prefix + variable length)      ...
/// +-----------------------------------------------------------+
/// ```
///
/// # System Paths
/// ```text
/// +-------------------+-------------------+-----------------------+
/// | Path type (1 bit) | Protocol (5 bits) | Address Type (2 bits) |
/// +-------------------+-------------------+-----------------------+----------------+
/// |                   Address (4/16/ * bytes)                  ...| Port (2 bytes) |
/// +---------------------------------------------------------------+----------------+
/// ```
impl Serialisable for SystemPath {
    fn ser_id(&self) -> SerId {
        serialisation_ids::SYSTEM_PATH
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
        let header = SystemPathHeader::from_system(self);
        header.put_into(buf);

        system_path_put_into_buf(self, buf)?;

        Ok(())
    }

    fn local(self: Box<Self>) -> Result<Box<dyn Any + Send>, Box<dyn Serialisable>> {
        unimplemented!()
    }
}

#[inline(always)]
fn system_path_put_into_buf(path: &SystemPath, buf: &mut dyn BufMut) -> Result<(), SerError> {
    match *path.address() {
        IpAddr::V4(ref ip) => buf.put_slice(&ip.octets()),
        IpAddr::V6(ref ip) => buf.put_slice(&ip.octets()),
        // TODO support named Domain
    }
    buf.put_u16(path.port());
    Ok(())
}
#[inline(always)]
fn system_path_from_buf(buf: &mut dyn Buf) -> Result<(SystemPathHeader, SystemPath), SerError> {
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
    let port = buf.get_u16();
    let system_path = SystemPath::new(header.protocol, address, port);
    Ok((header, system_path))
}

impl Deserialiser<SystemPath> for SystemPath {
    const SER_ID: SerId = serialisation_ids::SYSTEM_PATH;

    fn deserialise(buf: &mut dyn Buf) -> Result<SystemPath, SerError> {
        system_path_from_buf(buf).map(|t| t.1)
    }
}

impl Serialisable for ActorPath {
    fn ser_id(&self) -> SerId {
        serialisation_ids::ACTOR_PATH
    }

    // Returns the total size for this actor path, including system path information.
    // Returns `None` if `ActorPath::Named` and the name overflows the designated 2 bytes.
    fn size_hint(&self) -> Option<usize> {
        let mut size: usize = 0;
        size += self.system().size_hint()?; // def. returns Some

        size += match *self {
            ActorPath::Unique(_) => {
                // UUIDs are 16 bytes long (see [UuidBytes])
                16
            }
            ActorPath::Named(ref np) => {
                // Named paths are length-prefixed (2 bytes)
                // followed by variable-length name
                // TODO replace this with a heuristic once https://github.com/kompics/kompact/issues/33 is resolved
                let path_len: u16 = 2;
                //let name_len = np.path_ref().join("/").len();
                let name_len = np.path_ref().iter().fold(0usize, |acc, elem| {
                    let elem_len = elem.bytes().len();
                    acc.checked_add(elem_len).unwrap_or(std::usize::MAX)
                });
                let name_len = u16::try_from(name_len).ok()?;
                path_len.checked_add(name_len)? as usize
            }
        };
        Some(size)
    }

    /// Serializes a Unique or Named actor path.
    fn serialise(&self, buf: &mut dyn BufMut) -> Result<(), SerError> {
        // System Path
        let header = SystemPathHeader::from_path(&self);
        header.put_into(buf);
        system_path_put_into_buf(self.system(), buf)?;

        // Actor Path
        match self {
            ActorPath::Unique(up) => {
                let uuid = up.uuid_ref();
                buf.put_slice(uuid.as_bytes())
            }
            ActorPath::Named(np) => {
                let path = np.path_ref().join("/");
                let data = path.as_bytes();
                let name_len: u16 = u16::try_from(data.len()).map_err(|_| {
                    SerError::InvalidData("Named path overflows designated 2 bytes length.".into())
                })?;
                buf.put_u16(name_len);
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
    const SER_ID: SerId = serialisation_ids::ACTOR_PATH;

    fn deserialise(buf: &mut dyn Buf) -> Result<ActorPath, SerError> {
        let (header, system_path) = system_path_from_buf(buf)?;

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
                    ActorPath::Unique(UniquePath::with_system(system_path, uuid))
                }
            }
            PathType::Named => {
                let name_len = buf.get_u16() as usize;
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
                        ActorPath::Named(NamedPath::with_system(system_path, path))
                    }
                }
            }
        };
        Ok(path)
    }
}

#[cfg(test)]
mod serialisation_tests {
    use super::*;
    use crate::actors::SystemField;
    use bytes::BytesMut; //IntoBuf

    #[test]
    fn system_path_serequiv() {
        use super::{PathType, SystemPathHeader};
        use crate::{
            actors::{ActorPath, NamedPath, SystemPath, Transport},
            messaging::framing::AddressType,
        };

        let system_path = SystemPath::new(Transport::TCP, "127.0.0.1".parse().unwrap(), 8080u16);
        let named_path = ActorPath::Named(NamedPath::with_system(
            system_path.clone(),
            vec!["actor-name".into()],
        ));
        let unique_path =
            ActorPath::Unique(UniquePath::with_system(system_path.clone(), Uuid::new_v4()));
        {
            let header = SystemPathHeader::from_path(&named_path);
            assert_eq!(header.path_type, PathType::Named);
            assert_eq!(header.protocol, Transport::TCP);
            assert_eq!(header.address_type, AddressType::IPv4);
        }
        {
            let header = SystemPathHeader::from_path(&unique_path);
            assert_eq!(header.path_type, PathType::Unique);
            assert_eq!(header.protocol, Transport::TCP);
            assert_eq!(header.address_type, AddressType::IPv4);
        }

        let mut buf = BytesMut::with_capacity(system_path.size_hint().unwrap());
        system_path
            .serialise(&mut buf)
            .expect("SystemPath should serialise!");

        //let mut buf = buf.into();
        let deserialised =
            SystemPath::deserialise(&mut buf).expect("SystemPath should deserialise!");

        assert_eq!(system_path, deserialised);
    }

    #[test]
    fn actor_path_serequiv() {
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
            let size = Serialisable::size_hint(&unique_path).expect("Paths should have size hints");
            let mut buf = BytesMut::with_capacity(size);
            Serialisable::serialise(&unique_path, &mut buf)
                .expect("UUID ActorPath Serialisation should succeed");

            // Deserialise
            //let mut buf: Buf = buf.into();
            let deser_path = ActorPath::deserialise(&mut buf)
                .expect("UUID ActorPath Deserialisation should succeed");
            assert_eq!(buf.len(), 0);
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
            let size = Serialisable::size_hint(&named_path).expect("Paths should have size hints");
            let mut buf = BytesMut::with_capacity(size);
            Serialisable::serialise(&named_path, &mut buf)
                .expect("Named ActorPath Serialisation should succeed");

            // Deserialise
            let mut buf = buf.to_bytes();
            let deser_path = ActorPath::deserialise(&mut buf)
                .expect("Named ActorPath Deserialisation should succeed");
            assert_eq!(buf.len(), 0);
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

use super::*;

use std::convert::TryInto;

/// Contains constants with internal serialisation ids.
///
/// Currently ids between 0 and 20 are in internal use in Kompact.
pub mod serialisation_ids {
    use super::SerId;

    /// Id for a trivial deserialiser.
    pub const UNKNOWN: SerId = 0; // this is used for trivial deserialisers

    /// Id for a `DispatchEnvelope`.
    pub const DISPATCH_ENVELOPE: SerId = 1;

    /// Id for a `UniquePath`
    pub const UNIQUE_PATH: SerId = 2;

    /// Id for a `NamedPath`.
    pub const NAMED_PATH: SerId = 3;
    //pub const SYSTEM_PATH_HEADER: SerId = 4; // this one is freed now...reuse in th future!

    /// Id for a `SystemPath`.
    pub const SYSTEM_PATH: SerId = 5;

    /// Id for an [ActorPath](crate::prelude::ActorPath).
    pub const ACTOR_PATH: SerId = 6;

    /// Id for a `String` serialiser.
    pub const STR: SerId = 6;

    /// Id for a `u64` serialiser.
    pub const U64: SerId = 7;

    /// Id for a `()` (unit type) serialiser.
    pub const UNIT: SerId = 8;

    /// Id for the Serde serialiser
    pub const SERDE: SerId = 19;

    /// Id for the protobuf serialiser
    pub const PBUF: SerId = 20;
}

impl Deserialiser<Never> for Never {
    const SER_ID: SerId = serialisation_ids::UNKNOWN;

    fn deserialise(_buf: &mut dyn Buf) -> Result<Never, SerError> {
        unreachable!("Can't instantiate Never type, so can't deserialise it either");
    }
}

impl Serialisable for String {
    fn ser_id(&self) -> SerId {
        serialisation_ids::STR
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.as_bytes().len() + 8)
    }

    fn serialise(&self, buf: &mut dyn BufMut) -> Result<(), SerError> {
        let slice = self.as_bytes();
        let len_u64 = slice.len() as u64;
        buf.put_u64(len_u64);
        buf.put_slice(slice);
        Ok(())
    }

    fn local(self: Box<Self>) -> Result<Box<dyn Any + Send>, Box<dyn Serialisable>> {
        Ok(self)
    }
}
impl Deserialiser<String> for String {
    const SER_ID: SerId = serialisation_ids::STR;

    fn deserialise(buf: &mut dyn Buf) -> Result<String, SerError> {
        let len_u64 = buf.get_u64();
        let len: usize = len_u64.try_into().map_err(SerError::from_debug)?;
        // This approach is memory safe, but not overly efficient and also an attack vector for OOM attacks.
        // If you need different guarantees, write a different String serde implementation, that fulfills them
        let mut data: Vec<u8> = vec![0; len];
        buf.copy_to_slice(data.as_mut_slice());
        let s = String::from_utf8(data).map_err(SerError::from_debug)?;
        Ok(s)
    }
}

impl Serialisable for u64 {
    fn ser_id(&self) -> SerId {
        serialisation_ids::U64
    }

    fn size_hint(&self) -> Option<usize> {
        Some(8)
    }

    fn serialise(&self, buf: &mut dyn BufMut) -> Result<(), SerError> {
        buf.put_u64(*self);
        Ok(())
    }

    fn local(self: Box<Self>) -> Result<Box<dyn Any + Send>, Box<dyn Serialisable>> {
        Ok(self)
    }
}
impl Deserialiser<u64> for u64 {
    const SER_ID: SerId = serialisation_ids::U64;

    fn deserialise(buf: &mut dyn Buf) -> Result<u64, SerError> {
        let num = buf.get_u64();
        Ok(num)
    }
}

impl Serialisable for () {
    fn ser_id(&self) -> SerId {
        serialisation_ids::UNIT
    }

    fn size_hint(&self) -> Option<usize> {
        Some(0)
    }

    fn serialise(&self, _buf: &mut dyn BufMut) -> Result<(), SerError> {
        Ok(())
    }

    fn local(self: Box<Self>) -> Result<Box<dyn Any + Send>, Box<dyn Serialisable>> {
        Ok(self)
    }
}
impl Deserialiser<()> for () {
    const SER_ID: SerId = serialisation_ids::UNIT;

    fn deserialise(_buf: &mut dyn Buf) -> Result<(), SerError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::BytesMut;

    #[test]
    fn test_string_serialisation() {
        let test_str: String = "Test string with more than 16 characters".into();
        let mut mbuf = if let Some(size_hint) = test_str.size_hint() {
            BytesMut::with_capacity(size_hint)
        } else {
            BytesMut::with_capacity(16)
        };
        test_str.serialise(&mut mbuf).expect("serialise");
        let mut buf = mbuf.freeze();
        let res = String::deserialise(&mut buf);
        match res {
            Ok(test_str_res) => assert_eq!(test_str, test_str_res),
            Err(e) => panic!("{}", e),
        }
    }

    #[test]
    fn test_u64_serialisation() {
        let test_num: u64 = 123456789u64;
        let mut mbuf = if let Some(size_hint) = test_num.size_hint() {
            BytesMut::with_capacity(size_hint)
        } else {
            BytesMut::with_capacity(4)
        };
        test_num.serialise(&mut mbuf).expect("serialise");
        let mut buf = mbuf.freeze();
        let res = u64::deserialise(&mut buf);
        match res {
            Ok(test_num_res) => assert_eq!(test_num, test_num_res),
            Err(e) => panic!("{}", e),
        }
    }

    #[test]
    #[allow(clippy::unit_arg)]
    fn test_unit_serialisation() {
        let mut mbuf = if let Some(size_hint) = ().size_hint() {
            BytesMut::with_capacity(size_hint)
        } else {
            panic!("Unit serialiser should have produced a size hint");
        };
        ().serialise(&mut mbuf).expect("serialise");
        let mut buf = mbuf.freeze();
        let res = <()>::deserialise(&mut buf);
        res.expect("Should have produced a unit value")
    }
}

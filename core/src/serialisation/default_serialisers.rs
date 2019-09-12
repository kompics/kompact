use super::*;

/// Currently in internal use: 0-20
pub mod serialisation_ids {
    use super::SerId;

    pub const UNKNOWN: SerId = 0; // this is used for trivial deserialisers
    pub const DISPATCH_ENVELOPE: SerId = 1;
    pub const UNIQUE_PATH: SerId = 2;
    pub const NAMED_PATH: SerId = 3;
    //pub const SYSTEM_PATH_HEADER: SerId = 4; // this one is freed now...reuse in th future!
    pub const SYSTEM_PATH: SerId = 5;
    pub const ACTOR_PATH: SerId = 6;

    pub const STR: SerId = 6;
    pub const U64: SerId = 7;

    pub const PBUF: SerId = 20;
}

impl Serialisable for &'static str {
    fn ser_id(&self) -> SerId {
        serialisation_ids::STR
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.as_bytes().len())
    }

    fn serialise(&self, buf: &mut dyn BufMut) -> Result<(), SerError> {
        buf.put_slice(self.as_bytes());
        Ok(())
    }

    fn local(self: Box<Self>) -> Result<Box<dyn Any + Send>, Box<dyn Serialisable>> {
        Ok(self)
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
        buf.put_u64_be(*self);
        Ok(())
    }

    fn local(self: Box<Self>) -> Result<Box<dyn Any + Send>, Box<dyn Serialisable>> {
        Ok(self)
    }
}
// TODO finish later
//pub struct SerdeSerialiser<S>
//where
//    S: Serializer,
//{
//    type_id: u64,
//    size_hint: Option<usize>,
//    format: S,
//}
//
//impl<S> SerdeSerialiser<S>
//where
//    S: Serializer,
//{
//    pub fn new(type_id: u64, format: S) -> SerdeSerializer<S> {
//        SerdeSerialiser {
//            type_id,
//            size_hint: None,
//            format,
//        }
//    }
//
//    pub fn with_hint(type_id: u64, size_hint: usize, format: S) -> SerdeSerializer<S> {
//        SerdeSerialiser {
//            type_id,
//            size_hint: Some(size_hint),
//            format,
//        }
//    }
//}
//
//impl<T, S> Serialiser<T> for SerdeSerialiser<S>
//where
//    T: Serialize,
//    S: Serializer,
//{
//    fn id(&self) -> u64 {
//        self.type_id
//    }
//    fn size_hint(&self) -> Option<usize> {
//        self.size_hint
//    }
//    fn serialise(&self, v: &T, buf: &mut BufMut) -> Result<(), SerError> {
//        let encoded = v.serialize(self.format);
//    }
//}

//impl<T> From<T> for Box<Serialisable>
//where
//    T: Serialize + Debug + 'static,
//{
//    fn from(t: T) -> Self {
//        let sv = SerialisableValue { v: t, ser: t.1 };
//        Box::new(sv) as Box<Serialisable>
//    }
//}

use super::*;

/// Currently in internal use: 0-20
pub mod serialisation_ids {
    // 0 ???
    // 1-5 in messaging::framing::SerIdents
    pub const STR: u64 = 6;
    pub const U64: u64 = 7;

    pub const PBUF: u64 = 20;
}

impl Serialisable for &'static str {
    fn serid(&self) -> u64 {
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
    fn serid(&self) -> u64 {
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

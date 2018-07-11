use super::*;
use bytes::BufMut;
use std::any::Any;

mod serialisation_ids {
    pub const STR: u64 = 5;
}

impl Serialisable for &'static str {
    fn serid(&self) -> u64 {
        serialisation_ids::STR
    }
    fn size_hint(&self) -> Option<usize> {
        Some(self.as_bytes().len())
    }
    fn serialise(&self, buf: &mut BufMut) -> Result<(), SerError> {
        buf.put_slice(self.as_bytes());
        Ok(())
    }
    fn local(self: Box<Self>) -> Result<Box<Any + Send>, Box<Serialisable>> {
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

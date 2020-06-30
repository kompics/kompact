//! Provides serialisation support for Serde
use super::*;
use serde::{
    de::{
        self,
        DeserializeOwned,
        DeserializeSeed,
        EnumAccess,
        MapAccess,
        SeqAccess,
        VariantAccess,
        Visitor,
    },
    *,
};
use std::convert::TryInto;

/// Serialiser type for Serde enabled types
///
/// Must be specified explictly, to avoid ambiguity with types serialised in other ways
pub struct Serde;

impl<T> Serialiser<T> for Serde
where
    T: Serialize + SerialisationId + Debug + Send + 'static,
{
    fn ser_id(&self) -> SerId {
        T::SER_ID
    }

    fn size_hint(&self) -> Option<usize> {
        Some(std::mem::size_of::<T>()) // best guess
    }

    fn serialise(&self, v: &T, buf: &mut dyn BufMut) -> Result<(), SerError> {
        let serializer = BufSerializer::with(buf);
        v.serialize(serializer)
    }
}

impl<T> Deserialiser<T> for Serde
where
    T: DeserializeOwned + SerialisationId,
{
    const SER_ID: SerId = T::SER_ID;

    fn deserialise(buf: &mut dyn Buf) -> Result<T, SerError> {
        let mut deserializer = BufDeserializer::from_buf(buf);
        let t = T::deserialize(&mut deserializer)?;
        Ok(t)
    }
}

struct BufSerializer<'a> {
    buffer: &'a mut dyn BufMut,
}
impl<'a> BufSerializer<'a> {
    fn with(buffer: &'a mut dyn BufMut) -> Self {
        BufSerializer { buffer }
    }

    fn reborrow(&mut self) -> BufSerializer<'_> {
        BufSerializer {
            buffer: &mut *self.buffer,
        }
    }
}
impl<'a> Serializer for BufSerializer<'a> {
    type Error = SerError;
    type Ok = ();
    type SerializeMap = Self;
    type SerializeSeq = Self;
    type SerializeStruct = Self;
    type SerializeStructVariant = Self;
    type SerializeTuple = Self;
    type SerializeTupleStruct = Self;
    type SerializeTupleVariant = Self;

    fn serialize_bool(self, v: bool) -> Result<Self::Ok, Self::Error> {
        let num = if v { 1u8 } else { 0u8 };
        self.buffer.put_u8(num);
        Ok(())
    }

    fn serialize_i8(self, v: i8) -> Result<Self::Ok, Self::Error> {
        self.buffer.put_i8(v);
        Ok(())
    }

    fn serialize_i16(self, v: i16) -> Result<Self::Ok, Self::Error> {
        self.buffer.put_i16(v);
        Ok(())
    }

    fn serialize_i32(self, v: i32) -> Result<Self::Ok, Self::Error> {
        self.buffer.put_i32(v);
        Ok(())
    }

    fn serialize_i64(self, v: i64) -> Result<Self::Ok, Self::Error> {
        self.buffer.put_i64(v);
        Ok(())
    }

    fn serialize_u8(self, v: u8) -> Result<Self::Ok, Self::Error> {
        self.buffer.put_u8(v);
        Ok(())
    }

    fn serialize_u16(self, v: u16) -> Result<Self::Ok, Self::Error> {
        self.buffer.put_u16(v);
        Ok(())
    }

    fn serialize_u32(self, v: u32) -> Result<Self::Ok, Self::Error> {
        self.buffer.put_u32(v);
        Ok(())
    }

    fn serialize_u64(self, v: u64) -> Result<Self::Ok, Self::Error> {
        self.buffer.put_u64(v);
        Ok(())
    }

    fn serialize_f32(self, v: f32) -> Result<Self::Ok, Self::Error> {
        self.buffer.put_f32(v);
        Ok(())
    }

    fn serialize_f64(self, v: f64) -> Result<Self::Ok, Self::Error> {
        self.buffer.put_f64(v);
        Ok(())
    }

    fn serialize_char(self, v: char) -> Result<Self::Ok, Self::Error> {
        let num: u32 = v as u32;
        self.buffer.put_u32(num);
        Ok(())
    }

    fn serialize_str(self, v: &str) -> Result<Self::Ok, Self::Error> {
        let len = v.len();
        self.buffer.put_u64(len as u64);
        let bytes = v.as_bytes();
        self.buffer.put_slice(bytes);
        Ok(())
    }

    fn serialize_bytes(self, v: &[u8]) -> Result<Self::Ok, Self::Error> {
        let len = v.len();
        self.buffer.put_u64(len as u64);
        self.buffer.put_slice(v);
        Ok(())
    }

    fn serialize_none(self) -> Result<Self::Ok, Self::Error> {
        self.buffer.put_u8(0);
        Ok(())
    }

    fn serialize_some<T>(self, value: &T) -> Result<Self::Ok, Self::Error>
    where
        T: ?Sized + Serialize,
    {
        self.buffer.put_u8(1); // mark the some as filled
        value.serialize(self)
    }

    fn serialize_unit(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }

    fn serialize_unit_struct(self, _name: &'static str) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        variant_index: u32,
        _variant: &'static str,
    ) -> Result<Self::Ok, Self::Error> {
        self.buffer.put_u32(variant_index);
        Ok(())
    }

    // As is done here, serializers are encouraged to treat newtype structs as
    // insignificant wrappers around the data they contain.
    fn serialize_newtype_struct<T>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error>
    where
        T: ?Sized + Serialize,
    {
        value.serialize(self)
    }

    // Note that newtype variant (and all of the other variant serialization
    // methods) refer exclusively to the "externally tagged" enum
    // representation.
    fn serialize_newtype_variant<T>(
        self,
        _name: &'static str,
        variant_index: u32,
        _variant: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error>
    where
        T: ?Sized + Serialize,
    {
        self.buffer.put_u32(variant_index);
        value.serialize(self)
    }

    // Now we get to the serialization of compound types.
    //
    // The start of the sequence, each value, and the end are three separate
    // method calls. This one is responsible only for serializing the start.
    //
    // The length of the sequence may or may not be known ahead of time. This
    // doesn't make a difference in JSON because the length is not represented
    // explicitly in the serialized form. Some serializers may only be able to
    // support sequences for which the length is known up front.
    fn serialize_seq(self, len: Option<usize>) -> Result<Self::SerializeSeq, SerError> {
        let len = len.ok_or_else(|| {
            SerError::InvalidData("Sequence length must be known ahead of time!".into())
        })?;
        self.buffer.put_u64(len as u64);
        Ok(self)
    }

    // Some formats may be able to
    // represent tuples more efficiently by omitting the length, since tuple
    // means that the corresponding `Deserialize implementation will know the
    // length without needing to look at the serialized data.
    fn serialize_tuple(self, _len: usize) -> Result<Self::SerializeTuple, SerError> {
        Ok(self)
    }

    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleStruct, SerError> {
        Ok(self)
    }

    // Again this method is only responsible for the externally tagged representation.
    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        variant_index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleVariant, SerError> {
        self.buffer.put_u32(variant_index);
        Ok(self)
    }

    // Maps are represented as sequences of key-value pairs
    fn serialize_map(self, len: Option<usize>) -> Result<Self::SerializeMap, SerError> {
        let len = len.ok_or_else(|| {
            SerError::InvalidData("Map length must be known ahead of time!".into())
        })?;
        self.buffer.put_u64(len as u64);
        Ok(self)
    }

    // Other formats may be able to
    // omit the field names when serializing structs because the corresponding
    // Deserialize implementation is required to know what the keys are without
    // looking at the serialized data.
    fn serialize_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStruct, SerError> {
        Ok(self)
    }

    // This is the externally tagged representation.
    fn serialize_struct_variant(
        self,
        _name: &'static str,
        variant_index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStructVariant, SerError> {
        self.buffer.put_u32(variant_index);
        Ok(self)
    }
}

// The following 7 impls deal with the serialization of compound types like
// sequences and maps. Serialization of such types is begun by a Serializer
// method and followed by zero or more calls to serialize individual elements of
// the compound type and one call to end the compound type.
//
// This impl is SerializeSeq so these methods are called after `serialize_seq`
// is called on the Serializer.
impl<'a> ser::SerializeSeq for BufSerializer<'a> {
    // Must match the `Error` type of the serializer.
    type Error = SerError;
    // Must match the `Ok` type of the serializer.
    type Ok = ();

    // Serialize a single element of the sequence.
    fn serialize_element<T>(&mut self, value: &T) -> Result<Self::Ok, Self::Error>
    where
        T: ?Sized + Serialize,
    {
        value.serialize(self.reborrow())
    }

    // Close the sequence.
    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
}

// Same thing but for tuples.
impl<'a> ser::SerializeTuple for BufSerializer<'a> {
    type Error = SerError;
    type Ok = ();

    fn serialize_element<T>(&mut self, value: &T) -> Result<Self::Ok, Self::Error>
    where
        T: ?Sized + Serialize,
    {
        value.serialize(self.reborrow())
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
}

// Same thing but for tuple structs.
impl<'a> ser::SerializeTupleStruct for BufSerializer<'a> {
    type Error = SerError;
    type Ok = ();

    fn serialize_field<T>(&mut self, value: &T) -> Result<Self::Ok, Self::Error>
    where
        T: ?Sized + Serialize,
    {
        value.serialize(self.reborrow())
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
}

// Same for tuple variants.
impl<'a> ser::SerializeTupleVariant for BufSerializer<'a> {
    type Error = SerError;
    type Ok = ();

    fn serialize_field<T>(&mut self, value: &T) -> Result<Self::Ok, Self::Error>
    where
        T: ?Sized + Serialize,
    {
        value.serialize(self.reborrow())
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
}

// Some `Serialize` types are not able to hold a key and value in memory at the
// same time so `SerializeMap` implementations are required to support
// `serialize_key` and `serialize_value` individually.
//
// There is a third optional method on the `SerializeMap` trait. The
// `serialize_entry` method allows serializers to optimize for the case where
// key and value are both available simultaneously.
impl<'a> ser::SerializeMap for BufSerializer<'a> {
    type Error = SerError;
    type Ok = ();

    // The Serde data model allows map keys to be any serializable type. JSON
    // only allows string keys so the implementation below will produce invalid
    // JSON if the key serializes as something other than a string.
    //
    // A real JSON serializer would need to validate that map keys are strings.
    // This can be done by using a different Serializer to serialize the key
    // (instead of `&mut **self`) and having that other serializer only
    // implement `serialize_str` and return an error on any other data type.
    fn serialize_key<T>(&mut self, key: &T) -> Result<Self::Ok, Self::Error>
    where
        T: ?Sized + Serialize,
    {
        key.serialize(self.reborrow())
    }

    // It doesn't make a difference whether the colon is printed at the end of
    // `serialize_key` or at the beginning of `serialize_value`. In this case
    // the code is a bit simpler having it here.
    fn serialize_value<T>(&mut self, value: &T) -> Result<Self::Ok, Self::Error>
    where
        T: ?Sized + Serialize,
    {
        value.serialize(self.reborrow())
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
}

// Structs are like maps in which the keys are constrained to be compile-time
// constant strings.
impl<'a> ser::SerializeStruct for BufSerializer<'a> {
    type Error = SerError;
    type Ok = ();

    fn serialize_field<T>(&mut self, _key: &'static str, value: &T) -> Result<Self::Ok, Self::Error>
    where
        T: ?Sized + Serialize,
    {
        value.serialize(self.reborrow())
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
}

// More of the same
impl<'a> ser::SerializeStructVariant for BufSerializer<'a> {
    type Error = SerError;
    type Ok = ();

    fn serialize_field<T>(&mut self, _key: &'static str, value: &T) -> Result<Self::Ok, Self::Error>
    where
        T: ?Sized + Serialize,
    {
        value.serialize(self.reborrow())
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
}

impl ser::Error for SerError {
    fn custom<T: std::fmt::Display>(msg: T) -> Self {
        SerError::Unknown(msg.to_string())
    }
}

impl de::Error for SerError {
    fn custom<T: std::fmt::Display>(msg: T) -> Self {
        SerError::Unknown(msg.to_string())
    }
}

struct BufDeserializer<'de> {
    buffer: &'de mut dyn Buf,
}

impl<'de> BufDeserializer<'de> {
    fn from_buf(buffer: &'de mut dyn Buf) -> Self {
        BufDeserializer { buffer }
    }

    fn counting<'a>(&'a mut self, len: usize) -> ElementCounting<'a, 'de> {
        ElementCounting::new(self, len)
    }

    // fn as_enum<'a>(&'a mut self) -> Enum<'a, 'de> {
    //     Enum::new(self)
    // }

    // fn reborrow<'a: 'de>(&'a mut self) -> BufDeserializer<'a> {
    //     BufDeserializer {
    //         buffer: &mut *self.buffer,
    //     }
    // }
}

impl<'de, 'a> de::Deserializer<'de> for &'a mut BufDeserializer<'de> {
    type Error = SerError;

    // Look at the input data to decide what Serde data model type to
    // deserialize as. Not all data formats are able to support this operation.
    // Formats that support `deserialize_any` are known as self-describing.
    fn deserialize_any<V>(self, _visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        unimplemented!("Unsupported");
    }

    fn deserialize_bool<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let num = self.buffer.get_u8();
        let v = num != 0u8; // treat everything except 0 as true
        visitor.visit_bool(v)
    }

    fn deserialize_i8<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let num = self.buffer.get_i8();
        visitor.visit_i8(num)
    }

    fn deserialize_i16<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let num = self.buffer.get_i16();
        visitor.visit_i16(num)
    }

    fn deserialize_i32<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let num = self.buffer.get_i32();
        visitor.visit_i32(num)
    }

    fn deserialize_i64<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let num = self.buffer.get_i64();
        visitor.visit_i64(num)
    }

    fn deserialize_u8<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let num = self.buffer.get_u8();
        visitor.visit_u8(num)
    }

    fn deserialize_u16<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let num = self.buffer.get_u16();
        visitor.visit_u16(num)
    }

    fn deserialize_u32<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let num = self.buffer.get_u32();
        visitor.visit_u32(num)
    }

    fn deserialize_u64<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let num = self.buffer.get_u64();
        visitor.visit_u64(num)
    }

    fn deserialize_f32<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let num = self.buffer.get_f32();
        visitor.visit_f32(num)
    }

    fn deserialize_f64<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let num = self.buffer.get_f64();
        visitor.visit_f64(num)
    }

    fn deserialize_char<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let num = self.buffer.get_u32();
        let v = std::char::from_u32(num).ok_or_else(|| {
            SerError::Unknown(format!("Number {} does not represent a valid char!", num))
        })?;
        visitor.visit_char(v)
    }

    // Refer to the "Understanding deserializer lifetimes" page for information
    // about the three deserialization flavors of strings in Serde.
    fn deserialize_str<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_string(visitor)
    }

    fn deserialize_string<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let len_u64 = self.buffer.get_u64();
        let len: usize = len_u64.try_into().map_err(SerError::from_debug)?;
        // This approach is memory safe, but not overly efficient and also an attack vector for OOM attacks.
        // If you need different guarantees, write a different String serde implementation, that fulfills them
        let mut data: Vec<u8> = vec![0; len];
        self.buffer.copy_to_slice(data.as_mut_slice());
        let s = String::from_utf8(data).map_err(SerError::from_debug)?;
        visitor.visit_string(s)
    }

    fn deserialize_bytes<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        // only support owned bytes
        self.deserialize_byte_buf(visitor)
    }

    fn deserialize_byte_buf<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let len_u64 = self.buffer.get_u64();
        let len: usize = len_u64.try_into().map_err(SerError::from_debug)?;
        // This approach is memory safe, but not overly efficient and also an attack vector for OOM attacks.
        // If you need different guarantees, write a different serde implementation, that fulfills them
        let mut data: Vec<u8> = vec![0; len];
        self.buffer.copy_to_slice(data.as_mut_slice());
        visitor.visit_byte_buf(data)
    }

    fn deserialize_option<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let num = self.buffer.get_u8();
        if num == 0 {
            visitor.visit_none()
        } else {
            visitor.visit_some(self)
        }
    }

    // In Serde, unit means an anonymous value containing no data.
    fn deserialize_unit<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_unit()
    }

    // Unit struct means a named value containing no data.
    fn deserialize_unit_struct<V>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_unit()
    }

    // As is done here, serializers are encouraged to treat newtype structs as
    // insignificant wrappers around the data they contain. That means not
    // parsing anything other than the contained value.
    fn deserialize_newtype_struct<V>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_newtype_struct(self)
    }

    // Deserialization of compound types like sequences and maps happens by
    // passing the visitor an "Access" object that gives it the ability to
    // iterate through the data contained in the sequence.
    fn deserialize_seq<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let len_u64 = self.buffer.get_u64();
        let len: usize = len_u64.try_into().map_err(SerError::from_debug)?;
        visitor.visit_seq(self.counting(len))
    }

    // As indicated by the length parameter, the `Deserialize` implementation
    // for a tuple in the Serde data model is required to know the length of the
    // tuple before even looking at the input data.
    fn deserialize_tuple<V>(self, len: usize, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_seq(self.counting(len))
    }

    fn deserialize_tuple_struct<V>(
        self,
        _name: &'static str,
        len: usize,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_seq(self.counting(len))
    }

    // Much like `deserialize_seq` but calls the visitors `visit_map` method
    // with a `MapAccess` implementation, rather than the visitor's `visit_seq`
    // method with a `SeqAccess` implementation.
    fn deserialize_map<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let len_u64 = self.buffer.get_u64();
        let len: usize = len_u64.try_into().map_err(SerError::from_debug)?;
        visitor.visit_map(self.counting(len))
    }

    // Notice the `fields` parameter - a "struct" in the Serde data model means
    // that the `Deserialize` implementation is required to know what the fields
    // are before even looking at the input data. Any key-value pairing in which
    // the fields cannot be known ahead of time is probably a map.
    fn deserialize_struct<V>(
        self,
        _name: &'static str,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_seq(self.counting(fields.len()))
    }

    fn deserialize_enum<V>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_enum(self)
    }

    // An identifier in Serde is the type that identifies a field of a struct or
    // the variant of an enum.
    fn deserialize_identifier<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_u32(visitor)
    }

    // Like `deserialize_any` but indicates to the `Deserializer` that it makes
    // no difference which `Visitor` method is called because the data is
    // ignored.
    //
    // Some deserializers are able to implement this more efficiently than
    // `deserialize_any`, for example by rapidly skipping over matched
    // delimiters without paying close attention to the data in between.
    //
    // Some formats are not able to implement this at all. Formats that can
    // implement `deserialize_any` and `deserialize_ignored_any` are known as
    // self-describing.
    fn deserialize_ignored_any<V>(self, _visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        unimplemented!("Unsupported");
    }
}

struct ElementCounting<'a, 'de: 'a> {
    buffer: &'a mut BufDeserializer<'de>,
    len: usize,
    count: usize,
}

impl<'a, 'de> ElementCounting<'a, 'de> {
    fn new(buffer: &'a mut BufDeserializer<'de>, len: usize) -> Self {
        ElementCounting {
            buffer,
            len,
            count: 0usize,
        }
    }
}

// `SeqAccess` is provided to the `Visitor` to give it the ability to iterate
// through elements of the sequence.
impl<'de, 'a> SeqAccess<'de> for ElementCounting<'a, 'de> {
    type Error = SerError;

    fn next_element_seed<T>(&mut self, seed: T) -> Result<Option<T::Value>, Self::Error>
    where
        T: DeserializeSeed<'de>,
    {
        if self.count == self.len {
            Ok(None)
        } else {
            self.count += 1;
            seed.deserialize(&mut *self.buffer).map(Some)
        }
    }
}

// `MapAccess` is provided to the `Visitor` to give it the ability to iterate
// through entries of the map.
impl<'de, 'a> MapAccess<'de> for ElementCounting<'a, 'de> {
    type Error = SerError;

    fn next_key_seed<K>(&mut self, seed: K) -> Result<Option<K::Value>, Self::Error>
    where
        K: DeserializeSeed<'de>,
    {
        if self.count == self.len {
            Ok(None)
        } else {
            self.count += 1;
            seed.deserialize(&mut *self.buffer).map(Some)
        }
    }

    fn next_value_seed<V>(&mut self, seed: V) -> Result<V::Value, Self::Error>
    where
        V: DeserializeSeed<'de>,
    {
        seed.deserialize(&mut *self.buffer)
    }
}

// struct Enum<'a, 'de: 'a> {
//     buffer: &'a mut BufDeserializer<'de>,
// }

// impl<'a, 'de> Enum<'a, 'de> {
//     fn new(buffer: &'a mut BufDeserializer<'de>) -> Self {
//         Enum { buffer }
//     }
// }

// `EnumAccess` is provided to the `Visitor` to give it the ability to determine
// which variant of the enum is supposed to be deserialized.
//
// Note that all enum deserialization methods in Serde refer exclusively to the
// "externally tagged" enum representation.
impl<'a, 'de: 'a> EnumAccess<'de> for &'a mut BufDeserializer<'de> {
    type Error = SerError;
    type Variant = Self;

    fn variant_seed<V>(self, seed: V) -> Result<(V::Value, Self::Variant), Self::Error>
    where
        V: DeserializeSeed<'de>,
    {
        // The seed will be deserializing itself from
        // the key of the map.
        let val = seed.deserialize(&mut *self)?;
        Ok((val, self))
    }
}

// `VariantAccess` is provided to the `Visitor` to give it the ability to see
// the content of the single variant that it decided to deserialize.
impl<'a, 'de: 'a> VariantAccess<'de> for &'a mut BufDeserializer<'de> {
    type Error = SerError;

    // If the `Visitor` expected this variant to be a unit variant, the input
    // should have been the plain string case handled in `deserialize_enum`.
    fn unit_variant(self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn newtype_variant_seed<T>(self, seed: T) -> Result<T::Value, Self::Error>
    where
        T: DeserializeSeed<'de>,
    {
        seed.deserialize(self)
    }

    fn tuple_variant<V>(self, len: usize, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        de::Deserializer::deserialize_tuple(self, len, visitor)
        //self.buffer.deserialize_tuple(len, visitor)
    }

    fn struct_variant<V>(
        self,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        de::Deserializer::deserialize_struct(self, "name isn't used anyway", fields, visitor)
        //self.buffer
        //    .deserialize_struct("name isn't used anyway", fields, visitor)
    }
}

impl SerialisationId for ActorPath {
    const SER_ID: SerId = <ActorPath as Deserialiser<ActorPath>>::SER_ID;
}
impl Serialize for ActorPath {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let size = self.size_hint().unwrap_or(0);
        let mut buf: Vec<u8> = Vec::with_capacity(size);
        ActorPath::serialise(self, &mut buf).map_err(ser::Error::custom)?;
        serializer.serialize_bytes(&buf)
    }
}
struct ActorPathVisitor;
impl<'de> Visitor<'de> for ActorPathVisitor {
    type Value = ActorPath;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a byte serialised ActorPath")
    }

    fn visit_byte_buf<E>(self, value: Vec<u8>) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        let mut slice: &[u8] = &value;
        let path = ActorPath::deserialise(&mut slice).map_err(de::Error::custom)?;
        Ok(path)
    }
}
impl<'de> Deserialize<'de> for ActorPath {
    fn deserialize<D>(deserializer: D) -> Result<ActorPath, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_byte_buf(ActorPathVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::BytesMut;

    #[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
    struct Test {
        int: u32,
        seq: Vec<String>,
    }
    impl SerialisationId for Test {
        const SER_ID: SerId = 12345;
    }

    #[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
    enum E {
        Unit,
        Newtype(u32),
        Tuple(u32, u32),
        Struct { a: u32 },
    }
    impl SerialisationId for E {
        const SER_ID: SerId = 12346;
    }

    fn ser_deser_roundtrip<T>(v: T) -> Result<T, SerError>
    where
        T: Serialize + DeserializeOwned + SerialisationId + Send + Debug + 'static,
    {
        let serialisable: SerialisableValue<_, _> = (v, Serde).into();
        let mut mbuf = if let Some(size_hint) = serialisable.size_hint() {
            BytesMut::with_capacity(size_hint)
        } else {
            panic!("Unit serialiser should have produced a size hint");
        };
        serialisable.serialise(&mut mbuf).expect("serialise");
        let mut buf = mbuf.freeze();
        Serde::deserialise(&mut buf)
    }

    #[test]
    fn test_struct() {
        let expected = Test {
            int: 1,
            seq: vec!["a".to_owned(), "b".to_owned()],
        };
        let ser = ser_deser_roundtrip(expected.clone()).unwrap();
        assert_eq!(expected, ser);
    }

    #[test]
    fn test_enum() {
        {
            let expected = E::Unit;
            let ser = ser_deser_roundtrip(expected.clone()).unwrap();
            assert_eq!(expected, ser);
        }

        {
            let expected = E::Newtype(1);
            let ser = ser_deser_roundtrip(expected.clone()).unwrap();
            assert_eq!(expected, ser);
        }

        {
            let expected = E::Tuple(1, 2);
            let ser = ser_deser_roundtrip(expected.clone()).unwrap();
            assert_eq!(expected, ser);
        }

        {
            let expected = E::Struct { a: 1 };
            let ser = ser_deser_roundtrip(expected.clone()).unwrap();
            assert_eq!(expected, ser);
        }
    }

    #[test]
    fn test_actorpath() {
        let expected: ActorPath = doctest_helpers::TEST_PATH.parse().unwrap();
        let ser = ser_deser_roundtrip(expected.clone()).unwrap();
        assert_eq!(expected, ser);
    }
}

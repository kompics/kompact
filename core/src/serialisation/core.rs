use super::*;

#[derive(Debug)]
pub enum SerError {
    InvalidData(String),
    InvalidType(String),
    Unknown(String),
}

pub trait Serialiser<T>: Send {
    fn ser_id(&self) -> SerId;
    fn size_hint(&self) -> Option<usize> {
        None
    }
    fn serialise(&self, v: &T, buf: &mut dyn BufMut) -> Result<(), SerError>;
}

pub trait Serialisable: Send + Debug {
    fn ser_id(&self) -> SerId;

    /// Provides a suggested serialized size in bytes if possible, returning None otherwise.
    fn size_hint(&self) -> Option<usize>;

    /// Serialises this object into `buf`, returning a `SerError` if unsuccessful.
    fn serialise(&self, buf: &mut dyn BufMut) -> Result<(), SerError>;

    fn local(self: Box<Self>) -> Result<Box<dyn Any + Send>, Box<dyn Serialisable>>;

    fn serialised(&self) -> Result<crate::messaging::Serialised, SerError> {
        crate::serialisation::helpers::serialise_to_serialised(self)
    }
}

impl<T, S> From<(T, S)> for Box<dyn Serialisable>
where
    T: Send + Debug + 'static,
    S: Serialiser<T> + 'static,
{
    fn from(t: (T, S)) -> Self {
        let sv = SerialisableValue { v: t.0, ser: t.1 };
        Box::new(sv) as Box<dyn Serialisable>
    }
}

impl<T> From<T> for Box<dyn Serialisable>
where
    T: Send + Debug + Serialisable + Sized + 'static,
{
    fn from(t: T) -> Self {
        Box::new(t) as Box<dyn Serialisable>
    }
}

struct SerialisableValue<T, S>
where
    T: Send + Debug,
    S: Serialiser<T>,
{
    v: T,
    ser: S,
}

impl<T, S> Serialisable for SerialisableValue<T, S>
where
    T: Send + Debug + 'static,
    S: Serialiser<T>,
{
    fn ser_id(&self) -> SerId {
        self.ser.ser_id()
    }

    fn size_hint(&self) -> Option<usize> {
        self.ser.size_hint()
    }

    fn serialise(&self, buf: &mut dyn BufMut) -> Result<(), SerError> {
        self.ser.serialise(&self.v, buf)
    }

    fn local(self: Box<Self>) -> Result<Box<dyn Any + Send>, Box<dyn Serialisable>> {
        let b: Box<dyn Any + Send> = Box::new(self.v);
        Ok(b)
    }
}

impl<T, S> Debug for SerialisableValue<T, S>
where
    T: Send + Debug,
    S: Serialiser<T>,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(
            f,
            "Serialisable(id={:?},size={:?}, value={:?})",
            self.ser.ser_id(),
            self.ser.size_hint(),
            self.v
        )
    }
}

pub trait Deserialiser<T>: Send {
    const SER_ID: SerId;
    fn deserialise(buf: &mut dyn Buf) -> Result<T, SerError>;
}

pub trait Deserialisable<T> {
    fn ser_id(&self) -> SerId;
    fn get_deserialised(self) -> Result<T, SerError>;
}

pub trait BoxDeserialisable<T> {
    fn boxed_ser_id(&self) -> SerId;
    fn boxed_get_deserialised(self: Box<Self>) -> Result<T, SerError>;
}

impl<T> Deserialisable<T> for Box<dyn BoxDeserialisable<T>> {
    fn ser_id(&self) -> SerId {
        self.boxed_ser_id()
    }

    fn get_deserialised(self) -> Result<T, SerError> {
        self.boxed_get_deserialised()
    }
}

// /// Trivial identity Deserialisable
// impl<T> Deserialisable<T> for T {
//     fn ser_id(&self) -> SerId {
//         serialisation_ids::UNKNOWN
//     }

//     fn get_deserialised(self) -> Result<T, SerError> {
//         Ok(self)
//     }
// }

// /// Trivial heap to Stack Deserialisable
// impl<T: Send> Deserialisable<T> for Box<T> {
//     fn ser_id(&self) -> SerId {
//         unimplemented!();
//     }

//     fn get_deserialised(self) -> Result<T, SerError> {
//         Ok(*self)
//     }
// }

use super::*;

#[derive(Debug)]
pub enum SerError {
    InvalidData(String),
    InvalidType(String),
    Unknown(String),
}

pub trait Serialiser<T>: Send {
    fn serid(&self) -> u64;
    fn size_hint(&self) -> Option<usize> {
        None
    }
    fn serialise(&self, v: &T, buf: &mut BufMut) -> Result<(), SerError>;
}

pub trait Serialisable: Send + Debug {
    fn serid(&self) -> u64;

    /// Provides a suggested serialized size in bytes if possible, returning None otherwise.
    fn size_hint(&self) -> Option<usize>;

    /// Serialises this object into `buf`, returning a `SerError` if unsuccessful.
    fn serialise(&self, buf: &mut BufMut) -> Result<(), SerError>;

    fn local(self: Box<Self>) -> Result<Box<Any + Send>, Box<Serialisable>>;
}

impl<T, S> From<(T, S)> for Box<Serialisable>
where
    T: Send + Debug + 'static,
    S: Serialiser<T> + 'static,
{
    fn from(t: (T, S)) -> Self {
        let sv = SerialisableValue { v: t.0, ser: t.1 };
        Box::new(sv) as Box<Serialisable>
    }
}

impl<T> From<T> for Box<Serialisable>
where
    T: Send + Debug + Serialisable + Sized + 'static,
{
    fn from(t: T) -> Self {
        Box::new(t) as Box<Serialisable>
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
    fn serid(&self) -> u64 {
        self.ser.serid()
    }
    fn size_hint(&self) -> Option<usize> {
        self.ser.size_hint()
    }
    fn serialise(&self, buf: &mut BufMut) -> Result<(), SerError> {
        self.ser.serialise(&self.v, buf)
    }
    fn local(self: Box<Self>) -> Result<Box<Any + Send>, Box<Serialisable>> {
        let b: Box<Any + Send> = Box::new(self.v);
        Ok(b)
    }
}

impl<T, S> Debug for SerialisableValue<T, S>
where
    T: Send + Debug,
    S: Serialiser<T>,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(
            f,
            "Serialisable(id={:?},size={:?}, value={:?})",
            self.ser.serid(),
            self.ser.size_hint(),
            self.v
        )
    }
}

pub trait Deserialiser<T>: Send {
    fn deserialise(buf: &mut Buf) -> Result<T, SerError>;
}

pub trait Deserialisable<T> {
    fn serid(&self) -> u64;
    fn get_deserialised(self) -> Result<T, SerError>;
}

pub trait BoxDeserialisable<T> {
    fn boxed_serid(&self) -> u64;
    fn boxed_get_deserialised(self: Box<Self>) -> Result<T, SerError>;
}

impl<T> Deserialisable<T> for Box<BoxDeserialisable<T>> {
    fn serid(&self) -> u64 {
        self.boxed_serid()
    }
    fn get_deserialised(self) -> Result<T, SerError> {
        self.boxed_get_deserialised()
    }
}

/// Trivial identity Deserialisable
impl<T> Deserialisable<T> for T {
    fn serid(&self) -> u64 {
        unimplemented!();
    }
    fn get_deserialised(self) -> Result<T, SerError> {
        Ok(self)
    }
}

/// Trivial heap to Stack Deserialisable
impl<T: Send> Deserialisable<T> for Box<T> {
    fn serid(&self) -> u64 {
        unimplemented!();
    }
    fn get_deserialised(self) -> Result<T, SerError> {
        Ok(*self)
    }
}

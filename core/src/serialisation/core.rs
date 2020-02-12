use super::*;

/// Erros that can be thrown during serialisation or deserialisation
#[derive(Debug)]
pub enum SerError {
    /// The data was invalid, corrupted, or otherwise not as expected
    InvalidData(String),
    /// The data represents the wrong type, or an unknown type
    InvalidType(String),
    /// Any other kind of error
    Unknown(String),
}

impl SerError {
    pub fn from_debug<E: Debug>(error: E) -> SerError {
        let msg = format!("Wrapped error: {:?}", error);
        SerError::Unknown(msg)
    }
}

/// A trait for types that can serialise data of type `T`.
pub trait Serialiser<T>: Send {
    /// The serialisation id for this serialiser.
    ///
    /// Serialisation ids are used to determine the deserialiser to use with a particular byte buffer.
    /// They are prepended to the actual serialised data and read first during deserialisation.
    /// Serialisation ids must be globally unique within a distributed Kompact system.
    fn ser_id(&self) -> SerId;

    /// An indicator how many bytes must be reserved in a buffer for a value to be serialsed into it with this serialiser.
    ///
    /// If the total size is unknown, `None` should be returned.
    ///
    /// Generally, size hints should be cheap to calculate, compared to the actual serialisation,
    /// since they are simply optimisations to avoid many small memory allocations during the serialisation process.
    fn size_hint(&self) -> Option<usize> {
        None
    }

    /// Serialise `v` into `buf`.
    ///
    /// Serialisation should produce a copy, and not consume the original value.
    ///
    /// Returns a [SerError](SerError) if unsuccessful.
    fn serialise(&self, v: &T, buf: &mut dyn BufMut) -> Result<(), SerError>;
}

/// A trait for items that can serialise themselves into a buffer.
pub trait Serialisable: Send + Debug {
    /// The serialisation id for this serialisabl.
    ///
    /// Serialisation ids are used to determine the deserialiser to use with a particular byte buffer.
    /// They are prepended to the actual serialised data and read first during deserialisation.
    /// Serialisation ids must be globally unique within a distributed Kompact system.
    fn ser_id(&self) -> SerId;

    /// An indicator how many bytes must be reserved in a buffer for a value to be serialsed into it with this serialiser.
    ///
    /// If the total size is unknown, `None` should be returned.
    ///
    /// Generally, size hints should be cheap to calculate, compared to the actual serialisation,
    /// since they are simply optimisations to avoid many small memory allocations during the serialisation process.
    fn size_hint(&self) -> Option<usize>;

    /// Serialises this object (`self`) into `buf`.
    ///
    /// Serialisation should produce a copy, and not consume the original value.
    ///
    /// Returns a [SerError](SerError) if unsuccessful.
    fn serialise(&self, buf: &mut dyn BufMut) -> Result<(), SerError>;

    /// Try move this object onto the heap for reflection, instead of serialising.
    ///
    /// Return the original object if the move fails, so that it can still be serialised.
    fn local(self: Box<Self>) -> Result<Box<dyn Any + Send>, Box<dyn Serialisable>>;

    /// Serialise with a one-off buffer.
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

/// A trait to deserialise values of type `T` from buffers.
pub trait Deserialiser<T>: Send {
    /// The serialisation id for which this deserialiser is to be invoked.
    const SER_ID: SerId;
    /// Try to deserialise a `T` from `buf`.
    ///
    /// Returns a [SerError](SerError) if unsuccessful.
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

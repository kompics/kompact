use super::*;

/// Errors that can be thrown during serialisation or deserialisation
#[derive(Debug)]
pub enum SerError {
    /// The data was invalid, corrupted, or otherwise not as expected
    InvalidData(String),
    /// The data represents the wrong type, or an unknown type
    InvalidType(String),
    /// The Buffer we're serializing into failed
    BufferError(String),
    /// No Buffer Available.
    ///
    /// Raised by actors attempting to send more messages than their Buffers allow
    /// or what the Network is able to handle.
    NoBuffersAvailable(String),
    /// Type can not be cloned
    NoClone,
    /// Any other kind of error
    Unknown(String),
}

impl SerError {
    /// Create a serialisation error from any kind of error that
    /// implements the [Debug](std::fmt::Debug) trait
    ///
    /// This always produces the [Unknown](SerError::Unknown) variant.
    pub fn from_debug<E: Debug>(error: E) -> SerError {
        let msg = format!("Wrapped error: {:?}", error);
        SerError::Unknown(msg)
    }
}

impl fmt::Display for SerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SerError::BufferError(s) => {
                write!(f, "An issue occurred with the serialisation buffers: {}", s)
            }
            SerError::InvalidData(s) => write!(
                f,
                "The provided data was not appropriate for (de-)serialisation: {}",
                s
            ),
            SerError::InvalidType(s) => write!(
                f,
                "The provided type was not appropriate for (de-)serialisation: {}",
                s
            ),
            SerError::NoClone => write!(
                f,
                "The provided type can not be cloned, but try_clone() was attempted"
            ),
            SerError::NoBuffersAvailable(s) => write!(
                f,
                "Serialising into a BufferPool with no available buffers: {}",
                s
            ),
            SerError::Unknown(s) => write!(f, "A serialisation error occurred: {}", s),
        }
    }
}

impl std::error::Error for SerError {}

/// A trait that acts like a stable `TypeId` for serialisation
///
/// Requires ids to be assigned manually in some consistent fashion
/// so they can be reliable compared between in different binaries and rust versions.
///
/// This trait is used with serialisers that can deal with a large number of types
/// but require some internal differentiation, such as [Serde](crate::serialisation::serde_serialisers),
/// for example.
pub trait SerialisationId {
    /// The serialisation id for this type
    const SER_ID: SerId;
}

/// A trait for types that can serialise data of type `T`
pub trait Serialiser<T>: Send + TryClone {
    /// The serialisation id for this serialiser
    ///
    /// Serialisation ids are used to determine the deserialiser to use with a particular byte buffer.
    /// They are prepended to the actual serialised data and read first during deserialisation.
    /// Serialisation ids must be globally unique within a distributed Kompact system.
    fn ser_id(&self) -> SerId;

    /// An indicator how many bytes must be reserved in a buffer for a value to be
    /// serialsed into it with this serialiser
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

    /// Produce a copy of `v`, if possible
    ///
    /// If it can't be done cheaply, simply return `None` (which is the default implementation).
    fn try_clone_data(&self, _v: &T) -> Option<T> {
        None
    }
}

/// A trait for values that can serialise themselves into a buffer
pub trait Serialisable: Send + Debug {
    /// The serialisation id for this serialisable
    ///
    /// Serialisation ids are used to determine the deserialiser to use with a particular byte buffer.
    /// They are prepended to the actual serialised data and read first during deserialisation.
    /// Serialisation ids must be globally unique within a *distributed* Kompact system.
    fn ser_id(&self) -> SerId;

    /// An indicator how many bytes must be reserved in a buffer for a value to be
    /// serialsed into it with this serialiser
    ///
    /// If the total size is unknown, `None` should be returned.
    ///
    /// Generally, size hints should be cheap to calculate, compared to the actual serialisation,
    /// since they are simply optimisations to avoid many small memory allocations during the serialisation process.
    fn size_hint(&self) -> Option<usize>;

    /// Serialises this object (`self`) into `buf`
    ///
    /// Serialisation should produce a copy, and not consume the original value.
    ///
    /// Returns a [SerError](SerError) if unsuccessful.
    fn serialise(&self, buf: &mut dyn BufMut) -> Result<(), SerError>;

    // TODO serialise owned...may need to rename some things here

    /// Try move this object onto the heap for reflection, instead of serialising
    ///
    /// Returns the original object if the move fails, so that it can still be serialised.
    fn local(self: Box<Self>) -> Result<Box<dyn Any + Send>, Box<dyn Serialisable>>;

    /// Serialise with a one-off buffer
    ///
    /// Calls [serialise](Serialisable::serialise) internally by default.
    fn serialised(&self) -> Result<crate::messaging::Serialised, SerError> {
        crate::serialisation::ser_helpers::serialise_to_serialised(self)
    }

    /// Try to produce a cheap in-memory copy of this
    ///
    /// The default implementation returns `None`.
    fn cloned(&self) -> Option<Box<dyn Serialisable>> {
        None
    }
}

/// Turns a pair of a [Serialiser](Serialiser) and value of it's type `T` into a
/// stack-allocated [Serialisable](Serialisable)
impl<T, S> From<(T, S)> for SerialisableValue<T, S>
where
    T: Send + Debug + 'static,
    S: Serialiser<T> + 'static,
{
    fn from(t: (T, S)) -> Self {
        SerialisableValue::from_tuple(t)
    }
}

/// Turns a pair of a [Serialiser](Serialiser) and value of it's type `T` into a
/// heap-allocated [Serialisable](Serialisable)
impl<T, S> From<(T, S)> for Box<dyn Serialisable>
where
    T: Send + Debug + 'static,
    S: Serialiser<T> + 'static,
{
    fn from(t: (T, S)) -> Self {
        let sv: SerialisableValue<T, S> = t.into();
        Box::new(sv) as Box<dyn Serialisable>
    }
}

/// Turns a stack-allocated [Serialisable](Serialisable) into a
/// heap-allocated [Serialisable](Serialisable)
impl<T> From<T> for Box<dyn Serialisable>
where
    T: Send + Debug + Serialisable + Sized + 'static,
{
    fn from(t: T) -> Self {
        Box::new(t) as Box<dyn Serialisable>
    }
}

/// A data type equivalent to a pair of value and a serialiser for it
pub struct SerialisableValue<T, S>
where
    T: Send + Debug,
    S: Serialiser<T>,
{
    /// The value to be serialised
    pub v: T,
    /// The serialise to use
    pub ser: S,
}
impl<T, S> SerialisableValue<T, S>
where
    T: Send + Debug + 'static,
    S: Serialiser<T>,
{
    /// Create a new Serialisable type from a `T` and a `Serialiser` for `T`
    pub fn new(v: T, ser: S) -> Self {
        SerialisableValue { v, ser }
    }

    /// Create a new `Serialisable` type from a pair of `T` and a `Serialiser` for `T`
    pub fn from_tuple(t: (T, S)) -> Self {
        Self::new(t.0, t.1)
    }
}

impl<T, S> Serialisable for SerialisableValue<T, S>
where
    T: Send + Debug + 'static,
    S: Serialiser<T> + 'static,
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

    fn cloned(&self) -> Option<Box<dyn Serialisable>> {
        self.ser.try_clone().ok().and_then(|ser| {
            self.ser.try_clone_data(&self.v).map(|v| {
                let b: Box<dyn Serialisable> = Box::new(SerialisableValue::new(v, ser));
                b
            })
        })
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

/// A trait to deserialise values of type `T` from buffers
///
/// There should be one deserialiser with the same serialisation id
/// for each [Serialiser](Serialiser) or [Serialisable](Serialisable)
/// implementation. It is recommended to implement both on the same type.
pub trait Deserialiser<T>: Send {
    /// The serialisation id for which this deserialiser is to be invoked
    const SER_ID: SerId;

    /// Try to deserialise a `T` from the given `buf`
    ///
    /// Returns a [SerError](SerError) if unsuccessful.
    fn deserialise(buf: &mut dyn Buf) -> Result<T, SerError>;
}

/// A trait for values that can get deserialised themselves
///
/// Can be used for serialisers that reuse already allocated memory
/// as the target, such as protocol buffers.
pub trait Deserialisable<T> {
    /// The serialisation id for which this deserialiser is to be invoked
    fn ser_id(&self) -> SerId;

    /// Try to deserialise this data into a `T`
    ///
    /// Returns a [SerError](SerError) if unsuccessful.
    fn get_deserialised(self) -> Result<T, SerError>;
}

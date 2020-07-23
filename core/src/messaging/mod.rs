//! Messaging types for sending and receiving messages between remote actors.

use crate::{
    actors::{ActorPath, DynActorRef, DynActorRefFactory, MessageBounds},
    net::{buffer::BufferEncoder, events::NetworkEvent},
    serialisation::{Deserialiser, SerError, SerId, Serialisable, Serialiser},
    utils,
};
use bytes::{Buf, Bytes};
use std::{any::Any, convert::TryFrom};
use uuid::Uuid;

use crate::{
    net::{buffer::ChunkLease, frames::FRAME_HEAD_LEN},
    serialisation::ser_helpers::deserialise_msg,
};
use std::ops::Deref;

pub mod framing;

/// An incoming message from the networking subsystem
///
/// Provides actor paths for the `sender` of the message and the
/// `receiver` in addition to the actual `data`.
///
/// It is recommend to use [try_deserialise](NetData::try_deserialise) or
/// [try_deserialise_unchecked](NetData::try_deserialise_unchecked) to unpack
/// the contained data to the appropriate type.
/// These methods abstract over whether or not the data is actually serialised
/// or simply heap-allocated.
#[derive(Debug)]
pub struct NetMessage {
    /// The sender of the message
    ///
    /// More concretely, this is the actor path that was supplied
    /// as a source by the original sender.
    pub sender: ActorPath,
    /// The receiver of the message
    ///
    /// More concretely, this is the actor path that was used
    /// as a destination for the message.
    /// In particular, of the message was sent to a named path
    /// for the current component, instead of the unique path,
    /// then this will be the named path.
    pub receiver: ActorPath,
    /// The actual data of the message
    pub data: NetData,
}

/// The data part of an incoming message from the networking subsystem
///
/// A `NetData` instance can either represent serialised data
/// or heap-allocated "reflected" data cast to `Box<Any>`.
/// This is because messages are "reflected" instead of serialised whenever possible
/// for messages sent to an [ActorPath](ActorPath) that turnes out to be in the same
/// [KompactSystem](crate::runtime::KompactSystem).
///
/// Whether serialised or heap-allocated, network data always comes with a
/// serialisation id of type [SerId](SerId) that can be used to identify the
/// underlying type with a simple lookup.
///
/// It is recommend to use [try_deserialise](NetData::try_deserialise) or
/// [try_deserialise_unchecked](NetData::try_deserialise_unchecked) to unpack
/// the contained data to the appropriate type, instead of accessing the `data` field directly.
/// These methods abstract over whether or not the data is actually serialised
/// or simply heap-allocated.
#[derive(Debug)]
pub struct NetData {
    /// The serialisation id of the data
    pub ser_id: SerId,
    /// The content of the data, which can be either heap allocated or serialised
    pub(crate) data: HeapOrSer,
}

/// Holder for data that is either heap-allocated or
/// or serialised.
#[derive(Debug)]
pub enum HeapOrSer {
    /// Data is heap-allocated and can be [downcast](std::boxed::Box::downcast)
    Boxed(Box<dyn Serialisable>),
    /// Data is serialised and must be [deserialised](Deserialiser::deserialise)
    Serialised(bytes::Bytes),
    /// Data is serialised in a pooled buffer and must be [deserialised](Deserialiser::deserialise)
    Pooled(ChunkLease),
}

impl NetMessage {
    /// Create a network message with heap-allocated data
    pub fn with_box(
        ser_id: SerId,
        sender: ActorPath,
        receiver: ActorPath,
        data: Box<dyn Serialisable>,
    ) -> NetMessage {
        NetMessage {
            sender,
            receiver,
            data: NetData::with(ser_id, HeapOrSer::Boxed(data)),
        }
    }

    /// Create a network message with serialised data
    pub fn with_bytes(
        ser_id: SerId,
        sender: ActorPath,
        receiver: ActorPath,
        data: Bytes,
    ) -> NetMessage {
        NetMessage {
            sender,
            receiver,
            data: NetData::with(ser_id, HeapOrSer::Serialised(data)),
        }
    }

    /// Create a network message with a ChunkLease, pooled buffers.
    /// For outgoing network messages the data inside the `data` should be both serialised and (framed)[crate::net::frames].
    pub fn with_chunk(
        ser_id: SerId,
        sender: ActorPath,
        receiver: ActorPath,
        data: ChunkLease,
    ) -> NetMessage {
        NetMessage {
            sender,
            receiver,
            data: NetData::with(ser_id, HeapOrSer::Pooled(data)),
        }
    }

    /// Try to deserialise the data into a value of type `T` wrapped into a message
    ///
    /// This method attempts to deserialise the contents into an
    /// instance of `T` using the [deserialiser](Deserialiser) `D`.
    /// It will only do so after verifying that `ser_id == D::SER_ID`.
    ///
    /// If the serialisation id does not match, this message is returned unaltered
    /// wrapped in an [UnpackError](UnpackError::NoIdMatch).
    ///
    /// # Example
    ///
    /// ```
    /// use kompact::prelude::*;
    /// # use kompact::doctest_helpers;
    /// use bytes::BytesMut;
    ///
    /// # let some_path: ActorPath = doctest_helpers::TEST_PATH.parse().expect("actor path");
    /// # let some_path2 = some_path.clone();
    ///
    /// let test_str = "Test me".to_string();
    /// // serialise the string
    /// let mut mbuf = BytesMut::with_capacity(test_str.size_hint().expect("size hint"));
    /// test_str.serialise(&mut mbuf).expect("serialise");
    /// // create a net message
    /// let buf = mbuf.freeze();
    /// let msg = NetMessage::with_bytes(String::SER_ID, some_path, some_path2, buf);
    /// // try to deserialise it again
    /// match msg.try_into_deserialised::<u64, u64>() {
    ///     Ok(_) => unreachable!("It's definitely not a u64..."),
    ///     Err(UnpackError::NoIdMatch(msg_again)) => {
    ///         match msg_again.try_into_deserialised::<String, String>() {
    ///             Ok(test_msg) => assert_eq!(test_str, test_msg.content),
    ///             Err(_) => unreachable!("It's definitely a string..."),
    ///         }   
    ///     }
    ///     Err(error) => panic!("Not the error we expected: {:?}", error),  
    /// }
    /// ```
    pub fn try_into_deserialised<T: 'static, D>(
        self,
    ) -> Result<DeserialisedMessage<T>, UnpackError<Self>>
    where
        D: Deserialiser<T>,
    {
        let NetMessage {
            sender,
            receiver,
            data,
        } = self;
        match data.try_deserialise::<T, D>() {
            Ok(t) => Ok(DeserialisedMessage::with(sender, receiver, t)),
            Err(e) => Err(match e {
                UnpackError::NoIdMatch(data) => UnpackError::NoIdMatch(NetMessage {
                    sender,
                    receiver,
                    data,
                }),
                UnpackError::NoCast(data) => UnpackError::NoCast(data),
                UnpackError::DeserError(e) => UnpackError::DeserError(e),
            }),
        }
    }

    /// Try to deserialise the data into a value of type `T`
    ///
    /// This method attempts to deserialise the contents into an
    /// instance of `T` using the [deserialiser](Deserialiser) `D`.
    /// It will only do so after verifying that `ser_id == D::SER_ID`.
    ///
    /// If the serialisation id does not match, this message is returned unaltered
    /// wrapped in an [UnpackError](UnpackError::NoIdMatch).
    ///
    /// # Example
    ///
    /// ```
    /// use kompact::prelude::*;
    /// # use kompact::doctest_helpers;
    /// use bytes::BytesMut;
    ///
    /// # let some_path: ActorPath = doctest_helpers::TEST_PATH.parse().expect("actor path");
    /// # let some_path2 = some_path.clone();
    ///
    /// let test_str = "Test me".to_string();
    /// // serialise the string
    /// let mut mbuf = BytesMut::with_capacity(test_str.size_hint().expect("size hint"));
    /// test_str.serialise(&mut mbuf).expect("serialise");
    /// // create a net message
    /// let buf = mbuf.freeze();
    /// let msg = NetMessage::with_bytes(String::SER_ID, some_path, some_path2, buf);
    /// // try to deserialise it again
    /// match msg.try_deserialise::<u64, u64>() {
    ///     Ok(_) => unreachable!("It's definitely not a u64..."),
    ///     Err(UnpackError::NoIdMatch(msg_again)) => {
    ///         match msg_again.try_deserialise::<String, String>() {
    ///             Ok(test_res) => assert_eq!(test_str, test_res),
    ///             Err(_) => unreachable!("It's definitely a string..."),
    ///         }   
    ///     }
    ///     Err(error) => panic!("Not the error we expected: {:?}", error),  
    /// }
    /// ```
    /// # Note
    ///
    /// If you need the sender or the receiver to be owned after deserialisation, either use
    /// `msg.data.try_deserialise<...>(...)` instead,
    /// or use [try_into_deserialised](NetMessage::try_into_deserialised).
    pub fn try_deserialise<T: 'static, D>(self) -> Result<T, UnpackError<Self>>
    where
        D: Deserialiser<T>,
    {
        if self.data.ser_id == D::SER_ID {
            self.try_deserialise_unchecked::<T, D>()
        } else {
            Err(UnpackError::NoIdMatch(self))
        }
    }

    /// Try to deserialise the data into a value of type `T`
    ///
    /// This method attempts to deserialise the contents into an
    /// instance of `T` using the [deserialiser](Deserialiser) `D`
    /// without checking the `ser_id` first for a match.
    ///
    /// Only use this, if you have already verified that `ser_id == D::SER_ID`!
    /// Otherwise use [try_deserialise](NetMessage::try_deserialise).
    ///
    /// # Example
    ///
    /// ```
    /// use kompact::prelude::*;
    /// # use kompact::doctest_helpers;
    /// use bytes::BytesMut;
    ///
    /// # let some_path: ActorPath = doctest_helpers::TEST_PATH.parse().expect("actor path");
    /// # let some_path2 = some_path.clone();
    ///
    /// let test_str = "Test me".to_string();
    /// // serialise the string
    /// let mut mbuf = BytesMut::with_capacity(test_str.size_hint().expect("size hint"));
    /// test_str.serialise(&mut mbuf).expect("serialise");
    /// // create a net message
    /// let buf = mbuf.freeze();
    /// let msg = NetMessage::with_bytes(String::SER_ID, some_path, some_path2, buf);
    /// // try to deserialise it again
    /// match msg.ser_id() {
    ///     &u64::SER_ID => unreachable!("It's definitely not a u64..."),
    ///     &String::SER_ID => {
    ///         let test_res = msg.try_deserialise_unchecked::<String, String>().expect("deserialised");
    ///         assert_eq!(test_str, test_res);
    ///     }
    ///     _ => unreachable!("It's definitely not...whatever this is..."),
    /// }
    /// ```
    ///
    /// # Note
    ///
    /// The [match_deser](match_deser!) macro generates code that is approximately equivalent to the example above
    /// with some nicer syntax.
    ///
    /// If you need the sender or the receiver to be owned after deserialisation, either use
    /// `msg.data.try_deserialise_unchecked<...>(...)` instead,
    /// or use [try_into_deserialised](NetMessage::try_into_deserialised).
    pub fn try_deserialise_unchecked<T: 'static, D>(self) -> Result<T, UnpackError<Self>>
    where
        D: Deserialiser<T>,
    {
        let NetMessage {
            sender,
            receiver,
            data,
        } = self;
        data.try_deserialise_unchecked::<T, D>()
            .map_err(|e| match e {
                UnpackError::NoIdMatch(data) => UnpackError::NoIdMatch(NetMessage {
                    sender,
                    receiver,
                    data,
                }),
                UnpackError::NoCast(data) => UnpackError::NoCast(data),
                UnpackError::DeserError(e) => UnpackError::DeserError(e),
            })
    }

    /// Returns a reference to the serialisation id of this message
    pub fn ser_id(&self) -> &SerId {
        &self.data.ser_id
    }

    // /// Returns a reference to the sender's actor path
    // pub fn sender(&self) -> &ActorPath {
    //     &self.sender
    // }

    // /// Returns a reference to the receiver's actor path
    // pub fn receiver(&self) -> &ActorPath {
    //     &self.receiver
    // }

    // /// Returns a reference to the data in this message
    // pub fn data(&self) -> &NetData {
    //     &self.data
    // }
}

impl NetData {
    /// Create a new data instance from a serialisation id and some allocated data
    pub fn with(ser_id: SerId, data: HeapOrSer) -> Self {
        NetData { ser_id, data }
    }

    /// Try to deserialise the data into a value of type `T`
    ///
    /// This method attempts to deserialise the contents into an
    /// instance of `T` using the [deserialiser](Deserialiser) `D`.
    /// It will only do so after verifying that `ser_id == D::SER_ID`.
    ///
    /// If the serialisation id does not match, this message is returned unaltered
    /// wrapped in an [UnpackError](UnpackError::NoIdMatch).
    ///
    /// # Example
    ///
    /// ```
    /// use kompact::prelude::*;
    /// # use kompact::doctest_helpers;
    /// use bytes::BytesMut;
    ///
    /// # let some_path: ActorPath = doctest_helpers::TEST_PATH.parse().expect("actor path");
    /// # let some_path2 = some_path.clone();
    ///
    /// let test_str = "Test me".to_string();
    /// // serialise the string
    /// let mut mbuf = BytesMut::with_capacity(test_str.size_hint().expect("size hint"));
    /// test_str.serialise(&mut mbuf).expect("serialise");
    /// // create a net message
    /// let buf = mbuf.freeze();
    /// let msg = NetMessage::with_bytes(String::SER_ID, some_path, some_path2, buf);
    /// // try to deserialise it again
    /// match msg.try_deserialise::<u64, u64>() {
    ///     Ok(_) => unreachable!("It's definitely not a u64..."),
    ///     Err(UnpackError::NoIdMatch(msg_again)) => {
    ///         match msg_again.try_deserialise::<String, String>() {
    ///             Ok(test_res) => assert_eq!(test_str, test_res),
    ///             Err(_) => unreachable!("It's definitely a string..."),
    ///         }   
    ///     }
    ///     Err(error) => panic!("Not the error we expected: {:?}", error),  
    /// }
    /// ```
    pub fn try_deserialise<T: 'static, D>(self) -> Result<T, UnpackError<Self>>
    where
        D: Deserialiser<T>,
    {
        if self.ser_id == D::SER_ID {
            self.try_deserialise_unchecked::<T, D>()
        } else {
            Err(UnpackError::NoIdMatch(self))
        }
    }

    /// Try to deserialise the data into a value of type `T`
    ///
    /// This method attempts to deserialise the contents into an
    /// instance of `T` using the [deserialiser](Deserialiser) `D`
    /// without checking the `ser_id` first for a match.
    ///
    /// Only use this, if you have already verified that `ser_id == D::SER_ID`!
    /// Otherwise use [try_deserialise](NetMessage::try_deserialise).
    ///
    /// # Example
    ///
    /// ```
    /// use kompact::prelude::*;
    /// # use kompact::doctest_helpers;
    /// use bytes::BytesMut;
    ///
    /// # let some_path: ActorPath = doctest_helpers::TEST_PATH.parse().expect("actor path");
    /// # let some_path2 = some_path.clone();
    ///
    /// let test_str = "Test me".to_string();
    /// // serialise the string
    /// let mut mbuf = BytesMut::with_capacity(test_str.size_hint().expect("size hint"));
    /// test_str.serialise(&mut mbuf).expect("serialise");
    /// // create a net message
    /// let buf = mbuf.freeze();
    /// let msg = NetMessage::with_bytes(String::SER_ID, some_path, some_path2, buf);
    /// // try to deserialise it again
    /// match msg.ser_id() {
    ///     &u64::SER_ID => unreachable!("It's definitely not a u64..."),
    ///     &String::SER_ID => {
    ///         let test_res = msg.data.try_deserialise_unchecked::<String, String>().expect("deserialised");
    ///         assert_eq!(test_str, test_res);
    ///     }
    ///     _ => unreachable!("It's definitely not...whatever this is..."),
    /// }
    /// ```
    ///
    /// # Note
    ///
    /// The [match_deser](match_deser!) macro generates code that is approximately equivalent to the example above
    /// with some nicer syntax.
    pub fn try_deserialise_unchecked<T: 'static, D>(self) -> Result<T, UnpackError<Self>>
    where
        D: Deserialiser<T>,
    {
        let NetData { ser_id, data } = self;
        match data {
            HeapOrSer::Boxed(boxed_ser) => {
                let b = boxed_ser.local().map_err(|_| {
                    UnpackError::DeserError(SerError::Unknown(format!(
                        "Serialisable with id={} can't be converted to local!",
                        ser_id
                    )))
                })?;
                b.downcast::<T>()
                    .map(|b| *b)
                    .map_err(|b| UnpackError::NoCast(b))
            }
            HeapOrSer::Serialised(mut bytes) => {
                D::deserialise(&mut bytes).map_err(UnpackError::DeserError)
            }
            HeapOrSer::Pooled(mut chunk) => {
                D::deserialise(&mut chunk).map_err(UnpackError::DeserError)
            }
        }
    }

    /// Returns a reference to the serialisation id of this data
    pub fn ser_id(&self) -> &SerId {
        &self.ser_id
    }
}

/// A deserialised variant of [NetMessage](NetMessage)
///
/// Can be obtained via the [try_into_deserialised](NetMessage::try_into_deserialised) function.
#[derive(Debug)]
pub struct DeserialisedMessage<T> {
    /// The sender of the message
    ///
    /// More concretely, this is the actor path that was supplied
    /// as a source by the original sender.
    pub sender: ActorPath,
    /// The receiver of the message
    ///
    /// More concretely, this is the actor path that was used
    /// as a destination for the message.
    /// In particular, of the message was sent to a named path
    /// for the current component, instead of the unique path,
    /// then this will be the named path.
    pub receiver: ActorPath,
    /// The actual deserialised content of the message
    pub content: T,
}

impl<T> DeserialisedMessage<T> {
    /// Create a new deserialised message
    pub fn with(sender: ActorPath, receiver: ActorPath, content: T) -> Self {
        DeserialisedMessage {
            sender,
            receiver,
            content,
        }
    }
}

/// An error that is thrown when deserialisation of a [NetMessage](NetMessage) is attempted.
#[derive(Debug)]
pub enum UnpackError<T> {
    /// `ser_id` did not match the given [Deserialiser](crate::serialisation::Deserialiser).
    NoIdMatch(T),
    /// `Box<dyn Any>::downcast()` failed.
    NoCast(Box<dyn Any + Send + 'static>),
    /// An error occurred during deserialisation of the buffer.
    ///
    /// This can not contain `T`, as the buffer may have been corrupted during the failed attempt.
    DeserError(SerError),
}

impl<T> UnpackError<T> {
    /// Retrieve the wrapped `T` instance, if any
    pub fn get(self) -> Option<T> {
        match self {
            UnpackError::NoIdMatch(t) => Some(t),
            UnpackError::NoCast(_) => None,
            UnpackError::DeserError(_) => None,
        }
    }

    /// Retrieve a wrapped any box, if there is any
    pub fn get_box(self) -> Option<Box<dyn Any + Send + 'static>> {
        match self {
            UnpackError::NoIdMatch(_) => None,
            UnpackError::NoCast(b) => Some(b),
            UnpackError::DeserError(_) => None,
        }
    }
}

/// An error that can occur during [actor path](ActorPath) registration
#[derive(Debug, PartialEq, Eq, Clone, Copy, Hash)]
pub enum RegistrationError {
    /// An actor path with the same name exists already
    DuplicateEntry,
    /// This kind of registration is unsupported by the system's dispatcher implementation
    Unsupported,
}

/// Convenience alias for the result of a path registration attempt
pub type RegistrationResult = Result<ActorPath, RegistrationError>;

/// A holder for different variants of how feedback for a registration can be provided
#[derive(Debug)]
pub enum RegistrationPromise {
    /// Provide feedback via fulfilling the promise
    Fulfil(utils::KPromise<RegistrationResult>),
    /// Do not provide feedback
    None,
}

/// Envelope representing an actor registration event
///
/// This is used for registering an [ActorRef](crate::prelude::ActorRef)
/// with an [ActorPath](crate::prelude::ActorPath) on a dispatcher.
#[derive(Debug)]
pub struct RegistrationEnvelope {
    /// A network-only reference to the registered actor
    pub actor: DynActorRef,
    /// The path we want to register
    pub path: PathResolvable,
    /// Allow existing registrations to be replaced by this registration
    ///
    /// If `false`, attempting to register an existing path will result in an error.
    pub update: bool,
    /// An optional feedback promise, which returns the newly registered actor path or an error
    pub promise: RegistrationPromise,
}

impl RegistrationEnvelope {
    /// Create a registration envelope without a promise for feedback
    pub fn basic(
        actor: &(impl DynActorRefFactory + ?Sized),
        path: PathResolvable,
        update: bool,
    ) -> RegistrationEnvelope {
        RegistrationEnvelope {
            actor: actor.dyn_ref(),
            path,
            update,
            promise: RegistrationPromise::None,
        }
    }

    /// Create a registration envelope using a promise for feedback
    pub fn with_promise(
        actor: &(impl DynActorRefFactory + ?Sized),
        path: PathResolvable,
        update: bool,
        promise: utils::KPromise<RegistrationResult>,
    ) -> RegistrationEnvelope {
        RegistrationEnvelope {
            actor: actor.dyn_ref(),
            path,
            update,
            promise: RegistrationPromise::Fulfil(promise),
        }
    }
}

/// Wrapper for serialised data with a serialisation id
///
/// The serialisastion id identifies the [Serialiser](Serialiser) used
/// to serialise the data, and should be used to match
/// the corresponding [Deserialiser](Deserialiser) as well.
#[derive(Debug)]
pub struct Serialised {
    /// The serialisation id of the serialiser used to serialise the data
    pub ser_id: SerId,
    /// The serialised bytes
    pub data: Bytes,
}

impl TryFrom<(SerId, Bytes)> for Serialised {
    type Error = SerError;

    fn try_from(t: (SerId, Bytes)) -> Result<Self, Self::Error> {
        Ok(Serialised {
            ser_id: t.0,
            data: t.1,
        })
    }
}

impl<T, S> TryFrom<(&T, &S)> for Serialised
where
    T: std::fmt::Debug,
    S: Serialiser<T>,
{
    type Error = SerError;

    fn try_from(t: (&T, &S)) -> Result<Self, Self::Error> {
        crate::serialisation::ser_helpers::serialiser_to_serialised(t.0, t.1)
    }
}
// Doesn't work due to: https://github.com/rust-lang/rust/issues/50133
// Use [serialise_to_serialised](crate::serialisation::helpers::serialise_to_serialised(ser))
// or the `dyn` version below instead.
// impl<S> TryFrom<&S> for Serialised
// where
//     S: Serialisable,
// {
//     type Error = SerError;

//     fn try_from(ser: &S) -> Result<Self, Self::Error> {
//         crate::serialisation::helpers::serialise_to_serialised(ser)
//     }
// }

impl TryFrom<&dyn Serialisable> for Serialised {
    type Error = SerError;

    fn try_from(ser: &dyn Serialisable) -> Result<Self, Self::Error> {
        crate::serialisation::ser_helpers::serialise_to_serialised(ser)
    }
}

impl<E> TryFrom<Result<Serialised, E>> for Serialised {
    type Error = E;

    fn try_from(res: Result<Serialised, E>) -> Result<Self, Self::Error> {
        res
    }
}

// TODO: Move framing from the pooled data creation to the Network dispatcher!
/// Wrapper used to wrap framed values for transfer to the Network thread
#[derive(Debug)]
pub enum SerialisedFrame {
    /// Variant for Bytes allocated anywhere
    Bytes(Bytes),
    /// Variant for the Pooled buffers
    Chunk(ChunkLease),
}

impl SerialisedFrame {
    /// Returns `true` the frame is of zero length
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns the number of bytes in this frame
    pub fn len(&self) -> usize {
        self.bytes().len()
    }

    /// Returns the data in this frame as a slice
    pub fn bytes(&self) -> &[u8] {
        match self {
            SerialisedFrame::Chunk(chunk) => chunk.bytes(),
            SerialisedFrame::Bytes(bytes) => bytes.bytes(),
        }
    }
}

/// An abstraction over lazy or eagerly serialised data sent to the dispatcher
#[derive(Debug)]
pub enum DispatchData {
    /// Lazily serialised variant – must still be serialised by the dispatcher or networking system
    Lazy(Box<dyn Serialisable>),
    /// Should be serialised and [framed](crate::net::frames).
    Serialised((ChunkLease, SerId)),
}

impl DispatchData {
    /// The serialisation id associated with the data
    pub fn ser_id(&self) -> SerId {
        match self {
            DispatchData::Lazy(ser) => ser.ser_id(),
            DispatchData::Serialised((_chunk, ser_id)) => *ser_id,
        }
    }

    /// Try to extract a network message from this data for local delivery
    ///
    /// This can fail, if the data can't be moved onto the heap, and serialisation
    /// also fails.
    pub fn into_local(self, src: ActorPath, dst: ActorPath) -> Result<NetMessage, SerError> {
        match self {
            DispatchData::Lazy(ser) => {
                let ser_id = ser.ser_id();
                Ok(NetMessage::with_box(ser_id, src, dst, ser))
                // match ser.local() {
                //     Ok(s) => Ok(NetMessage::with_box(ser_id, src, dst, s)),
                //     Err(ser) => crate::serialisation::ser_helpers::serialise_to_msg(src, dst, ser),
                // }
            }
            DispatchData::Serialised((mut chunk, _ser_id)) => {
                // The chunk contains the full frame, deserialize_msg does not deserialize FrameHead so we advance the read_pointer first
                chunk.advance(FRAME_HEAD_LEN as usize);
                //println!("to_local (from: {:?}; to: {:?})", src, dst);
                Ok(deserialise_msg(chunk).expect("s11n errors"))
            }
        }
    }

    /// Try to serialise this to data to bytes for remote delivery
    pub fn into_serialised(
        self,
        src: ActorPath,
        dst: ActorPath,
        buf: &mut BufferEncoder,
    ) -> Result<SerialisedFrame, SerError> {
        match self {
            DispatchData::Lazy(ser) => Ok(SerialisedFrame::Chunk(
                crate::serialisation::ser_helpers::serialise_msg(&src, &dst, ser.deref(), buf)?,
            )),
            DispatchData::Serialised((chunk, _ser_id)) => Ok(SerialisedFrame::Chunk(chunk)),
        }
    }
}

/// Envelope with messages for the system'sdispatcher
#[derive(Debug)]
pub enum DispatchEnvelope {
    /// A potential network message that must be resolved
    Msg {
        /// The source of the message
        src: ActorPath,
        /// The destination of the message
        dst: ActorPath,
        /// The actual data to be dispatched
        msg: DispatchData,
    },
    /// A message that may already be partially serialised
    ForwardedMsg {
        /// The message being forwarded
        msg: NetMessage,
    },
    /// A request for actor path registration
    Registration(RegistrationEnvelope),
    /// An event from the network
    Event(EventEnvelope),
}

/// An event from the network
///
/// This is left as an enum for future extension.
#[derive(Debug)]
pub enum EventEnvelope {
    /// An event from the network
    Network(NetworkEvent),
}

/// A message that is accepted by an actor's mailbox
#[derive(Debug)]
pub enum MsgEnvelope<M: MessageBounds> {
    /// A message of the actor's `Message` type
    Typed(M),
    /// A message from the network
    Net(NetMessage),
}

/// Something that can resolved to some kind of path by the dispatcher
#[derive(Debug, Clone)]
pub enum PathResolvable {
    /// An actual actor path
    Path(ActorPath),
    /// The unique id of an actor
    ///
    /// Can be resolved to a [unique path](ActorPath::Unique) in the current system.
    ActorId(Uuid),
    /// An actor path alias with all segments merged
    ///
    /// Can be resolved to a [named path](ActorPath::Named) in the current system.
    Alias(String),
    /// The system path (as provided by the dispatcher)
    System,
}

impl From<ActorPath> for PathResolvable {
    fn from(path: ActorPath) -> Self {
        PathResolvable::Path(path)
    }
}

/// A macro to make matching serialisation ids and deserialising easier
///
/// This macro basically generates a large match statement on the serialisation ids
/// and then uses [try_deserialise_unchecked](crate::prelude::NetMessage::try_deserialise_unchecked)
/// to get a value for the matched type, which it then uses to invoke the right-hand side
/// which expects that particular type.
///
/// # Basic Example
///
/// ```
/// use kompact::prelude::*;
/// # use kompact::doctest_helpers;
/// use bytes::BytesMut;
///
/// # let some_path: ActorPath = doctest_helpers::TEST_PATH.parse().expect("actor path");
/// # let some_path2 = some_path.clone();
///
/// let test_str = "Test me".to_string();
/// // serialise the string
/// let mut mbuf = BytesMut::with_capacity(test_str.size_hint().expect("size hint"));
/// test_str.serialise(&mut mbuf).expect("serialise");
/// // create a net message
/// let buf = mbuf.freeze();
/// let msg = NetMessage::with_bytes(String::SER_ID, some_path, some_path2, buf);
/// // try to deserialise it again
/// match_deser!(msg; {
///     _num: u64 [u64]           => unreachable!("It's definitely not a u64..."),
///     test_res: String [String] => assert_eq!(test_str, test_res),
/// });
/// ```
///
/// # Example with Error Handling
///
/// ```
/// use kompact::prelude::*;
/// # use kompact::doctest_helpers;
/// use bytes::BytesMut;
///
/// # let some_path: ActorPath = doctest_helpers::TEST_PATH.parse().expect("actor path");
/// # let some_path2 = some_path.clone();
///
/// let test_str = "Test me".to_string();
/// // serialise the string
/// let mut mbuf = BytesMut::with_capacity(test_str.size_hint().expect("size hint"));
/// test_str.serialise(&mut mbuf).expect("serialise");
/// // create a net message
/// let buf = mbuf.freeze();
/// let msg = NetMessage::with_bytes(String::SER_ID, some_path, some_path2, buf);
/// // try to deserialise it again
/// match_deser!(msg; {
///     _num: u64 [u64]           => unreachable!("It's definitely not a u64..."),
///     test_res: String [String] => assert_eq!(test_str, test_res),
///     !Err(error)               => panic!("Some error occurred during deserialisation: {:?}", error),
///     _                         => unreachable!("It's definitely not...whatever this is..."),
/// });
/// ```
#[macro_export]
macro_rules! match_deser {
    ($msg:expr ; { $($id:ident : $target_ty:ty [$deser:ty] => $rhs:expr),* , }) => {
        match $msg.ser_id() {
            $( &<$deser as $crate::prelude::Deserialiser<$target_ty>>::SER_ID => {
            let $id = $msg.try_deserialise_unchecked::<$target_ty, $deser>().unwrap();
            $rhs
        } )*,
        _ => unimplemented!(),
        }
    };
    ($msg:expr ; { $($id:ident : $target_ty:ty [$deser:ty] => $rhs:expr),* , !Err($e:pat) => $err_handler:expr, }) => {
        match $msg.ser_id() {
            $( &<$deser as $crate::prelude::Deserialiser<$target_ty>>::SER_ID => {
            match $msg.try_deserialise_unchecked::<$target_ty, $deser>() {
                Ok($id) => $rhs,
                Err($e) => $err_handler,
            }
        } )*,
        _ => unimplemented!(),
        }
    };
    ($msg:expr ; { $($id:ident : $target_ty:ty [$deser:ty] => $rhs:expr),* , _ => $other:expr, }) => {
        match $msg.ser_id() {
            $( &<$deser as $crate::prelude::Deserialiser<$target_ty>>::SER_ID => {
            let $id = $msg.try_deserialise_unchecked::<$target_ty, $deser>().unwrap();
            $rhs
        } )*,
        _ => $other,
        }
    };
    ($msg:expr ; { $($id:ident : $target_ty:ty [$deser:ty] => $rhs:expr),* , !Err($e:pat) => $err_handler:expr , _ => $other:expr, }) => {
        match $msg.ser_id() {
            $( &<$deser as $crate::prelude::Deserialiser<$target_ty>>::SER_ID => {
            match $msg.try_deserialise_unchecked::<$target_ty, $deser>() {
                Ok($id) => $rhs,
                Err($e) => $err_handler,
            }
        } )*,
        _ => $other,
        }
    };
}

#[cfg(test)]
mod deser_macro_tests {
    use super::*;
    use crate::serialisation::Serialiser;
    use bytes::{Buf, BufMut};
    use std::str::FromStr;

    #[test]
    fn simple_macro_test() {
        simple_macro_test_impl(|msg| {
            match_deser! { msg; {
                    res: MsgA [MsgA] => EitherAOrB::A(res),
                    res: MsgB [BSer] => EitherAOrB::B(res),
                }
            }
        })
    }

    #[test]
    fn simple_macro_test_with_other() {
        simple_macro_test_impl(|msg| {
            match_deser! { msg; {
                    res: MsgA [MsgA] => EitherAOrB::A(res),
                    res: MsgB [BSer] => EitherAOrB::B(res),
                    _ => unimplemented!("Should be either MsgA or MsgB!"),
                }
            }
        })
    }

    #[test]
    #[should_panic(expected = "test panic please ignore")]
    fn simple_macro_test_with_err() {
        simple_macro_test_impl(|msg| {
            match_deser! { msg; {
                    res: MsgA [MsgA] => EitherAOrB::A(res),
                    res: MsgB [BSer] => EitherAOrB::B(res),
                    !Err(_e) => panic!("test panic please ignore"),
                }
            }
        });
        simple_macro_test_err_impl(|msg| {
            match_deser! { msg; {
                    res: MsgA [MsgA] => EitherAOrB::A(res),
                    res: MsgB [BSer] => EitherAOrB::B(res),
                    !Err(_e) => panic!("test panic please ignore"),
                }
            }
        });
    }

    #[test]
    #[should_panic(expected = "test panic please ignore")]
    fn simple_macro_test_with_err_and_other() {
        simple_macro_test_impl(|msg| {
            match_deser! { msg; {
                    res: MsgA [MsgA] => EitherAOrB::A(res),
                    res: MsgB [BSer] => EitherAOrB::B(res),
                    !Err(_e) => panic!("test panic please ignore"),
                    _ => unimplemented!("Should be either MsgA or MsgB!"),
                }
            }
        });
        // alternative design
        //  simple_macro_test_impl(|msg| {
        //     match_deser! { msg; {
        //             Msg(res: MsgA [MsgA]) => EitherAOrB::A(res),
        //             Msg(res: MsgB [BSer]) => EitherAOrB::B(res),
        //             Err(_e) => panic!("test panic please ignore"),
        //             _ => unimplemented!("Should be either MsgA or MsgB!"),
        //         }
        //     }
        // });
        simple_macro_test_err_impl(|msg| {
            match_deser! { msg; {
                    res: MsgA [MsgA] => EitherAOrB::A(res),
                    res: MsgB [BSer] => EitherAOrB::B(res),
                    !Err(_e) => panic!("test panic please ignore"),
                    _ => unimplemented!("Should be either MsgA or MsgB!"),
                }
            }
        });
    }

    #[test]
    fn simple_no_macro_test() {
        simple_macro_test_impl(|msg| match msg.ser_id() {
            &MsgA::SER_ID => {
                let res = msg
                    .try_deserialise_unchecked::<MsgA, MsgA>()
                    .expect("MsgA should deserialise!");
                EitherAOrB::A(res)
            }
            &BSer::SER_ID => {
                let res = msg
                    .try_deserialise_unchecked::<MsgB, BSer>()
                    .expect("MsgB should deserialise!");
                EitherAOrB::B(res)
            }
            _ => unimplemented!("Should be either MsgA or MsgB!"),
        })
    }

    fn simple_macro_test_impl<F>(f: F)
    where
        F: Fn(NetMessage) -> EitherAOrB,
    {
        let ap = ActorPath::from_str("local://127.0.0.1:12345/testme").expect("an ActorPath");

        let msg_a = MsgA::new(54);
        let msg_b = MsgB::new(true);

        let msg_a_ser = crate::serialisation::ser_helpers::serialise_to_msg(
            ap.clone(),
            ap.clone(),
            Box::new(msg_a),
        )
        .expect("MsgA should serialise!");
        let msg_b_ser = crate::serialisation::ser_helpers::serialise_to_msg(
            ap.clone(),
            ap,
            (msg_b, BSer).into(),
        )
        .expect("MsgB should serialise!");
        assert!(f(msg_a_ser).is_a());
        assert!(f(msg_b_ser).is_b());
    }

    fn simple_macro_test_err_impl<F>(f: F)
    where
        F: Fn(NetMessage) -> EitherAOrB,
    {
        let ap = ActorPath::from_str("local://127.0.0.1:12345/testme").expect("an ActorPath");

        let msg = NetMessage::with_bytes(MsgA::SERID, ap.clone(), ap, Bytes::default());

        f(msg);
    }

    enum EitherAOrB {
        A(MsgA),
        B(MsgB),
    }

    impl EitherAOrB {
        fn is_a(&self) -> bool {
            match self {
                EitherAOrB::A(_) => true,
                EitherAOrB::B(_) => false,
            }
        }

        fn is_b(&self) -> bool {
            !self.is_a()
        }
    }

    #[derive(Debug)]
    struct MsgA {
        index: u64,
    }

    impl MsgA {
        const SERID: SerId = 42;

        fn new(index: u64) -> MsgA {
            MsgA { index }
        }
    }

    impl Serialisable for MsgA {
        fn ser_id(&self) -> SerId {
            Self::SERID
        }

        fn size_hint(&self) -> Option<usize> {
            Some(8)
        }

        fn serialise(&self, buf: &mut dyn BufMut) -> Result<(), SerError> {
            buf.put_u64(self.index);
            Ok(())
        }

        fn local(self: Box<Self>) -> Result<Box<dyn Any + Send>, Box<dyn Serialisable>> {
            Ok(self)
        }
    }

    impl Deserialiser<MsgA> for MsgA {
        const SER_ID: SerId = Self::SERID;

        fn deserialise(buf: &mut dyn Buf) -> Result<MsgA, SerError> {
            if buf.remaining() < 8 {
                return Err(SerError::InvalidData(
                    "Less than 8bytes remaining in buffer!".to_string(),
                ));
            }
            let index = buf.get_u64();
            let msg = MsgA { index };
            Ok(msg)
        }
    }

    #[derive(Debug)]
    struct MsgB {
        flag: bool,
    }

    impl MsgB {
        const SERID: SerId = 43;

        fn new(flag: bool) -> MsgB {
            MsgB { flag }
        }
    }

    struct BSer;

    impl Serialiser<MsgB> for BSer {
        fn ser_id(&self) -> SerId {
            MsgB::SERID
        }

        fn size_hint(&self) -> Option<usize> {
            Some(1)
        }

        fn serialise(&self, v: &MsgB, buf: &mut dyn BufMut) -> Result<(), SerError> {
            let num = if v.flag { 1u8 } else { 0u8 };
            buf.put_u8(num);
            Ok(())
        }
    }

    impl Deserialiser<MsgB> for BSer {
        const SER_ID: SerId = MsgB::SERID;

        fn deserialise(buf: &mut dyn Buf) -> Result<MsgB, SerError> {
            let num = buf.get_u8();
            let flag = num == 1u8;
            let msg = MsgB { flag };
            Ok(msg)
        }
    }
}

use super::*;
use crate::prelude::SessionId;

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
    /// When a connection is lost/closed and then re-established, new received messages will have an
    /// incremented session.
    ///
    /// The field is only modified by the Network-layer in incoming messages, and in
    /// `NetworkStatus`-messages.
    ///
    /// For any sequence of two messages received from a remote actor:
    ///     If the session of the messages differs, an intermediate message *may* have been lost.
    ///     Conversely, if the session does not differ no intermediate message was lost.
    pub session: SessionId,
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
    ChunkLease(ChunkLease),
    /// Data is serialised in a pooled buffer and must be [deserialised](Deserialiser::deserialise)
    ChunkRef(ChunkRef),
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
            session: SessionId::default(),
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
            session: SessionId::default(),
        }
    }

    /// Create a network message with a ChunkLease, pooled buffers.
    /// For outgoing network messages the data inside the `data` should be both serialised and (framed)[crate::net::frames].
    pub fn with_chunk_lease(
        ser_id: SerId,
        sender: ActorPath,
        receiver: ActorPath,
        data: ChunkLease,
    ) -> NetMessage {
        NetMessage {
            sender,
            receiver,
            data: NetData::with(ser_id, HeapOrSer::ChunkLease(data)),
            session: SessionId::default(),
        }
    }

    /// Create a network message with a ChunkLease, pooled buffers.
    /// For outgoing network messages the data inside the `data` should be both serialised and (framed)[crate::net::frames].
    pub fn with_chunk_ref(
        ser_id: SerId,
        sender: ActorPath,
        receiver: ActorPath,
        data: ChunkRef,
    ) -> NetMessage {
        NetMessage {
            sender,
            receiver,
            data: NetData::with(ser_id, HeapOrSer::ChunkRef(data)),
            session: SessionId::default(),
        }
    }

    /// Return a reference to the `sender` field
    pub fn sender(&self) -> &ActorPath {
        &self.sender
    }

    /// Returns the session of the `NetMessage`
    pub fn session(&self) -> SessionId {
        self.session
    }

    /// Sets the `session` of the `NetMessage`
    pub(crate) fn set_session(&mut self, session: SessionId) -> () {
        self.session = session;
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
            session,
        } = self;
        match data.try_deserialise::<T, D>() {
            Ok(t) => Ok(DeserialisedMessage::with(sender, receiver, t)),
            Err(e) => Err(match e {
                UnpackError::NoIdMatch(data) => UnpackError::NoIdMatch(NetMessage {
                    sender,
                    receiver,
                    data,
                    session,
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
            session,
        } = self;
        data.try_deserialise_unchecked::<T, D>()
            .map_err(|e| match e {
                UnpackError::NoIdMatch(data) => UnpackError::NoIdMatch(NetMessage {
                    sender,
                    receiver,
                    data,
                    session,
                }),
                UnpackError::NoCast(data) => UnpackError::NoCast(data),
                UnpackError::DeserError(e) => UnpackError::DeserError(e),
            })
    }

    /// Returns a reference to the serialisation id of this message
    pub fn ser_id(&self) -> &SerId {
        &self.data.ser_id
    }
}

impl TryClone for NetMessage {
    fn try_clone(&self) -> Result<Self, SerError> {
        self.data.try_clone().map(|data| NetMessage {
            sender: self.sender.clone(),
            receiver: self.receiver.clone(),
            data,
            session: self.session,
        })
    }
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
            HeapOrSer::ChunkLease(mut chunk) => {
                D::deserialise(&mut chunk).map_err(UnpackError::DeserError)
            }
            HeapOrSer::ChunkRef(mut chunk) => {
                D::deserialise(&mut chunk).map_err(UnpackError::DeserError)
            }
        }
    }

    /// Returns a reference to the serialisation id of this data
    pub fn ser_id(&self) -> &SerId {
        &self.ser_id
    }
}

impl TryClone for NetData {
    fn try_clone(&self) -> Result<Self, SerError> {
        self.data.try_clone().map(|data| NetData {
            ser_id: self.ser_id,
            data,
        })
    }
}

impl TryClone for HeapOrSer {
    fn try_clone(&self) -> Result<Self, SerError> {
        match self {
            HeapOrSer::Boxed(ser) => {
                let mut buf: Vec<u8> = Vec::new();
                let res = ser.serialise(&mut buf);
                res.map(|_| HeapOrSer::Serialised(Bytes::from(buf)))
            }
            HeapOrSer::Serialised(bytes) => Ok(HeapOrSer::Serialised(bytes.clone())),
            HeapOrSer::ChunkLease(chunk) => Ok(HeapOrSer::Serialised(chunk.create_byte_clone())),
            HeapOrSer::ChunkRef(chunk) => Ok(HeapOrSer::ChunkRef(chunk.clone())),
        }
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

use super::*;

/// An abstraction over lazy or eagerly serialised data sent to the dispatcher
#[derive(Debug)]
pub enum DispatchData {
    /// Lazily serialised variant – must still be serialised by the dispatcher or networking system
    Lazy(Box<dyn Serialisable>),
    /// Should be serialised and [framed](crate::net::frames::Frame).
    SerialisedLease(ChunkLease),
    /// Should be serialised and [framed](crate::net::frames::Frame).
    SerialisedRef(ChunkRef),
}

impl DispatchData {
    /// Try to extract a network message from this data for local delivery
    ///
    /// This can fail, if the data can't be moved onto the heap, and serialisation
    /// also fails.
    pub fn into_local(self, src: ActorPath, dst: ActorPath) -> Result<NetMessage, SerError> {
        match self {
            DispatchData::Lazy(ser) => {
                let ser_id = ser.ser_id();
                Ok(NetMessage::with_box(ser_id, src, dst, ser))
            }
            DispatchData::SerialisedLease(mut chunk) => {
                // The chunk contains the full frame, deserialize_msg does not deserialize FrameHead so we advance the read_pointer first
                chunk.advance(FRAME_HEAD_LEN as usize);
                //println!("to_local (from: {:?}; to: {:?})", src, dst);
                Ok(deserialise_chunk_lease(chunk).expect("s11n errors"))
            }
            DispatchData::SerialisedRef(mut chunk) => {
                // The chunk contains the full frame, deserialize_msg does not deserialize FrameHead so we advance the read_pointer first
                chunk.advance(FRAME_HEAD_LEN as usize);
                //println!("to_local (from: {:?}; to: {:?})", src, dst);
                Ok(deserialise_chunk_ref(chunk).expect("s11n errors"))
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
            DispatchData::Lazy(ser) => Ok(SerialisedFrame::ChunkLease(
                crate::serialisation::ser_helpers::serialise_msg(&src, &dst, ser.deref(), buf)?,
            )),
            DispatchData::SerialisedLease(chunk) => Ok(SerialisedFrame::ChunkLease(chunk)),
            DispatchData::SerialisedRef(chunk) => Ok(SerialisedFrame::ChunkRef(chunk)),
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
    /// Killed components send their BufferChunks to the Dispatcher for safe de-allocation
    LockedChunk(BufferChunk),
}

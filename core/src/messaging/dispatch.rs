use super::*;
use crate::{prelude::SessionId, serialisation::ser_helpers::deserialise_bytes};

/// An abstraction over lazy or eagerly serialised data sent to the dispatcher
#[derive(Debug)]
pub enum DispatchData {
    /// Lazily serialised variant – must still be serialised by the dispatcher or networking system
    /// (Serialisable Msg, Source, Destination)
    Lazy(Box<dyn Serialisable>, ActorPath, ActorPath),
    /// Should be serialised and [framed](crate::net::frames::Frame).
    Serialised(SerialisedFrame),
    /// Used in message forwarding
    NetMessage(NetMessage),
}

impl DispatchData {
    /// Try to extract a network message from this data for local delivery
    ///
    /// This can fail, if the data can't be moved onto the heap, and serialisation
    /// also fails.
    pub fn into_local(self) -> Result<NetMessage, SerError> {
        match self {
            DispatchData::Lazy(ser, src, dst) => {
                let ser_id = ser.ser_id();
                Ok(NetMessage::with_box(
                    ser_id,
                    src,
                    dst,
                    ser,
                    SessionId::LOCAL_SESSION,
                ))
            }
            DispatchData::Serialised(SerialisedFrame::ChunkLease(mut chunk_lease)) => {
                // The chunk contains the full frame, deserialize_msg does not deserialize FrameHead so we advance the read_pointer first
                chunk_lease.advance(FRAME_HEAD_LEN as usize);
                //println!("to_local (from: {:?}; to: {:?})", src, dst);
                Ok(
                    deserialise_chunk_lease(chunk_lease, SessionId::LOCAL_SESSION)
                        .expect("s11n errors"),
                )
            }
            DispatchData::Serialised(SerialisedFrame::ChunkRef(mut chunk_ref)) => {
                // The chunk contains the full frame, deserialize_msg does not deserialize FrameHead so we advance the read_pointer first
                chunk_ref.advance(FRAME_HEAD_LEN as usize);
                //println!("to_local (from: {:?}; to: {:?})", src, dst);
                Ok(
                    deserialise_chunk_ref(chunk_ref, SessionId::LOCAL_SESSION)
                        .expect("s11n errors"),
                )
            }
            DispatchData::Serialised(SerialisedFrame::Bytes(mut bytes)) => {
                bytes.advance(FRAME_HEAD_LEN as usize);
                //println!("to_local (from: {:?}; to: {:?})", src, dst);
                Ok(deserialise_bytes(bytes).expect("s11n errors"))
            }
            DispatchData::NetMessage(net_message) => Ok(net_message),
        }
    }

    /// Try to serialise this to data to bytes for remote delivery
    pub fn into_serialised(self, buf: &mut BufferEncoder) -> Result<SerialisedFrame, SerError> {
        match self {
            DispatchData::Lazy(ser, src, dst) => Ok(SerialisedFrame::ChunkLease(
                crate::serialisation::ser_helpers::serialise_msg(&src, &dst, ser.deref(), buf)?,
            )),
            DispatchData::Serialised(frame) => Ok(frame),
            DispatchData::NetMessage(net_message) => Ok(SerialisedFrame::ChunkRef(
                crate::serialisation::ser_helpers::embed_msg(net_message, buf)?,
            )),
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

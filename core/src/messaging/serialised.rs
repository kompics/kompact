use super::*;

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

/// Wrapper used to wrap framed values for transfer to the Network thread
#[derive(Debug)]
pub enum SerialisedFrame {
    /// Variant for Bytes allocated anywhere
    Bytes(Bytes),
    /// Variant for the Pooled buffers
    ChunkLease(ChunkLease),
    /// Variant for the Pooled buffers
    ChunkRef(ChunkRef),
}

impl SerialisedFrame {
    /// Returns `true` the frame is of zero length
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns the number of bytes in this frame
    pub fn len(&self) -> usize {
        match self {
            SerialisedFrame::ChunkLease(chunk) => chunk.remaining(),
            SerialisedFrame::ChunkRef(chunk) => chunk.remaining(),
            SerialisedFrame::Bytes(bytes) => bytes.remaining(),
        }
    }

    /// Returns the data in this frame as a slice, in case of chaining only the front is returned!
    pub fn bytes(&self) -> &[u8] {
        match self {
            SerialisedFrame::ChunkLease(chunk) => chunk.bytes(),
            SerialisedFrame::ChunkRef(chunk) => chunk.bytes(),
            SerialisedFrame::Bytes(bytes) => bytes.bytes(),
        }
    }

    /// Used by UDP sending which requires the frame to be a contiguous byte-sequence.
    /// Does nothing if it's already contiguous.
    pub fn make_contiguous(&mut self) {
        let len = self.len();
        if len > self.bytes().len() {
            match self {
                SerialisedFrame::ChunkLease(chunk) => {
                    *self = SerialisedFrame::Bytes(chunk.copy_to_bytes(len));
                }
                SerialisedFrame::ChunkRef(chunk) => {
                    *self = SerialisedFrame::Bytes(chunk.copy_to_bytes(len));
                }
                _ => {
                    // Unreachable
                    panic!("Impossible error, can't convert uncontiguous Bytes to contiguous");
                }
            }
        }
    }
}

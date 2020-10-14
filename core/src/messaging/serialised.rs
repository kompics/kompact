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

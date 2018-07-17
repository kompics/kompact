use bytes::{Buf, BufMut};
use std::any::Any;
use std::fmt;
use std::fmt::Debug;

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

pub mod helpers {
    use actors::ActorPath;
    use actors::SystemPath;
    use actors::UniquePath;
    use bytes::{Buf, Bytes, BytesMut};
    use messaging::framing::SerIdents;
    use messaging::MsgEnvelope;
    use messaging::ReceiveEnvelope;
    use serialisation::SerError;
    use serialisation::Serialisable;
    use uuid::Uuid;

    /// Creates a new `ReceiveEnvelope` from the provided fields, allocating a new `BytesMut`
    /// according to the message's size hint. The message's serialized data is stored in this buffer.
    pub fn serialise_to_recv_envelope(
        src: ActorPath,
        dst: ActorPath,
        msg: Box<Serialisable>,
    ) -> Result<MsgEnvelope, SerError> {
        if let Some(size) = msg.size_hint() {
            let mut buf = BytesMut::with_capacity(size);
            match msg.serialise(&mut buf) {
                Ok(_) => {
                    let envelope = MsgEnvelope::Receive(ReceiveEnvelope::Msg {
                        src,
                        dst,
                        ser_id: msg.serid(),
                        data: buf.freeze(),
                    });
                    Ok(envelope)
                }
                Err(ser_err) => Err(ser_err),
            }
        } else {
            Err(SerError::Unknown("Unknown serialisation size".into()))
        }
    }

    /// Serializes the provided actor paths and message into a new [BytesMut]
    ///
    /// # Format
    /// The serialized format is:
    ///     source          ActorPath
    ///         path_type   u8
    ///         path        `[u8; 16]` if [UniquePath], else a length-prefixed UTF-8 encoded string
    ///     destination     ActorPath
    ///         (see above for specific format)
    ///     ser_id          u64
    ///     message         raw bytes
    pub fn serialise_msg(
        src: &ActorPath,
        dst: &ActorPath,
        msg: Box<Serialisable>,
    ) -> Result<Bytes, SerError> {
        use bytes::BytesMut;
        let mut size: usize = 0;
        size += src.size_hint().unwrap_or(0);
        size += dst.size_hint().unwrap_or(0);
        size += Serialisable::size_hint(&msg.serid()).unwrap_or(0);
        size += msg.size_hint().unwrap_or(0);

        if size == 0 {
            return Err(SerError::InvalidData("Encoded size is zero".into()));
        }

        let mut buf = BytesMut::with_capacity(size);
        Serialisable::serialise(src, &mut buf)?;
        Serialisable::serialise(dst, &mut buf)?;
        Serialisable::serialise(&msg.serid(), &mut buf)?;
        Serialisable::serialise(msg.as_ref(), &mut buf)?;

        Ok(buf.freeze())
    }

    /// Extracts a [MsgEnvelope] from the provided buffer according to the format above.
    pub fn deserialise_msg<B: Buf>(
        mut buffer: B,
        system: &SystemPath,
    ) -> Result<ReceiveEnvelope, SerError> {

        if buffer.remaining() < 1 {
            return Err(SerError::InvalidData("Not enough bytes available".into()));
        }

        let (mut buffer, src) = deserialise_actor_path(&mut buffer, system)?;
        let (buffer, dst) = deserialise_actor_path(&mut buffer, system)?;
        let ser_id: u64 = buffer.get_u64();
        let data = buffer.bytes().into();

        let envelope = ReceiveEnvelope::Msg {
            src,
            dst,
            ser_id,
            data,
        };

        Ok(envelope)
    }

    fn deserialise_actor_path<B: Buf>(
        mut buf: B,
        system: &SystemPath,
    ) -> Result<(B, ActorPath), SerError> {
        let path_type: SerIdents = buf.get_u8().into();
        let (buf, path) = match path_type {
            SerIdents::UniqueActorPath => {
                let uuid_buf = buf.take(16);
                let uuid = Uuid::from_bytes(uuid_buf.bytes()).expect("UUID deser");
                let path = ActorPath::Unique(UniquePath::with_system(system.clone(), uuid));
                let mut buf = uuid_buf.into_inner();
                buf.advance(16);
                (buf, path)
            }
            _other => unimplemented!(),
        };
        Ok((buf, path))
    }
}

//struct BufDeserialisable<B: Buf, D: Deserialiser> {
//    buf: B,
//    deser:
//}

#[cfg(test)]
mod tests {

    use super::*;
    use bytes::{BytesMut, IntoBuf};

    #[derive(PartialEq, Debug, Clone)]
    struct Test1 {
        i: u64,
    }

    struct T1Ser;
    impl Serialiser<Test1> for T1Ser {
        fn serid(&self) -> u64 {
            1
        }
        fn size_hint(&self) -> Option<usize> {
            Some(88)
        }
        fn serialise(&self, v: &Test1, buf: &mut BufMut) -> Result<(), SerError> {
            //buf.put_u64::<BigEndian>(v.i);
            buf.put_u64(v.i);
            for i in 0..10 {
                buf.put_u64(i);
            }
            Result::Ok(())
        }
    }
    impl Deserialiser<Test1> for T1Ser {
        fn deserialise(buf: &mut Buf) -> Result<Test1, SerError> {
            let i = buf.get_u64();
            Result::Ok(Test1 { i })
        }
    }

    #[test]
    fn trivial_deser_equivalence() {
        let t1 = Test1 { i: 42 };
        let t1c = t1.clone();
        //let t1_des: Deserialisable<Test1> = t1;

        //assert_eq!(1, Deserialisable::<Test1>::id(&t1));

        match Deserialisable::<Test1>::get_deserialised(t1) {
            Ok(t2) => assert_eq!(t1c, t2),
            Err(e) => panic!(e),
        }

        let t1b = Box::new(t1c.clone());

        match Deserialisable::<Test1>::get_deserialised(t1b) {
            Ok(t2) => assert_eq!(t1c, t2),
            Err(e) => panic!(e),
        }
    }

    #[test]
    fn ser_deser_equivalence() {
        let t1 = Test1 { i: 42 };
        let mut mbuf = BytesMut::with_capacity(64);
        //let mut mbuf = vec![];
        let t1c = t1.clone();
        mbuf.reserve(T1Ser.size_hint().unwrap());
        T1Ser
            .serialise(&t1, &mut mbuf)
            .expect("should have serialised!");
        //println!("Serialised bytes: {:?}", mbuf);
        let mut buf = mbuf.into_buf();
        let t1_res = T1Ser::deserialise(&mut buf);
        match t1_res {
            Ok(t2) => assert_eq!(t1c, t2),
            Err(e) => panic!(e),
        }
        //        match Deserialisable::<Test1>::get(t1) {
        //            Ok(t2) => assert_eq!(t1c, t2),
        //            Err(e) => panic!(e),
        //        }  Err(e) => panic!(e),
        //        }
    }
}

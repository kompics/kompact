use bytes::{Buf, BufMut};
use std::{
    any::Any,
    fmt::{self, Debug},
};

use super::*;

mod core;
mod default_serialisers;
#[cfg(feature = "protobuf")]
pub mod protobuf_serialisers;
pub mod ser_helpers;
#[cfg(feature = "serde_support")]
#[allow(clippy::needless_lifetimes)]
pub mod serde_serialisers;

pub use self::{core::*, default_serialisers::*};

/// A trait that allows to determine the number of bytes in a `SerId`.
pub trait SerIdSize {
    /// The number of bytes in a concrete implementation of `SerId`.
    fn size(&self) -> usize;
}

#[cfg(feature = "ser_id_64")]
mod ser_id {
    use super::SerIdSize;
    use bytes::{Buf, BufMut};

    /// Type alias for the concrete implementation of serialisation ids.
    ///
    /// This version is activated by the `ser_id_64` feature flag, and uses a `u64` as the underlying size.
    pub type SerId = u64;

    impl SerIdSize for SerId {
        fn size(&self) -> usize {
            8
        }
    }

    /// A trait to retrieve a `SerId` from a buffer.
    pub trait SerIdBuf {
        /// Deserialises a `SerId` from this buffer.
        fn get_ser_id(&mut self) -> SerId;
    }

    /// A trait to put a `SerId` into a mutable buffer.
    pub trait SerIdBufMut {
        /// Serialises a `SerId` into this buffer.
        fn put_ser_id(&mut self, ser_id: SerId) -> ();
    }

    impl<B: Buf> SerIdBuf for B {
        fn get_ser_id(&mut self) -> SerId {
            self.get_u64()
        }
    }

    impl<B: BufMut> SerIdBufMut for B {
        fn put_ser_id(&mut self, ser_id: SerId) -> () {
            self.put_u64(ser_id)
        }
    }
}

#[cfg(feature = "ser_id_32")]
mod ser_id {
    use super::SerIdSize;
    use bytes::{Buf, BufMut};

    /// Type alias for the concrete implementation of serialisation ids.
    ///
    /// This version is activated by the `ser_id_32` feature flag, and uses a `u32` as the underlying size.
    pub type SerId = u32;

    impl SerIdSize for SerId {
        fn size(&self) -> usize {
            4
        }
    }

    /// A trait to retrieve a `SerId` from a buffer.
    pub trait SerIdBuf {
        /// Deserialises a `SerId` from this buffer.
        fn get_ser_id(&mut self) -> SerId;
    }

    /// A trait to put a `SerId` into a mutable buffer.
    pub trait SerIdBufMut {
        /// Serialises a `SerId` into this buffer.
        fn put_ser_id(&mut self, ser_id: SerId) -> ();
    }

    impl<B: Buf> SerIdBuf for B {
        fn get_ser_id(&mut self) -> SerId {
            self.get_u32()
        }
    }

    impl<B: BufMut> SerIdBufMut for B {
        fn put_ser_id(&mut self, ser_id: SerId) -> () {
            self.put_u32(ser_id)
        }
    }
}

#[cfg(feature = "ser_id_16")]
mod ser_id {
    use super::SerIdSize;
    use bytes::{Buf, BufMut};

    /// Type alias for the concrete implementation of serialisation ids.
    ///
    /// This version is activated by the `ser_id_16` feature flag, and uses a `u16` as the underlying size.
    pub type SerId = u16;

    impl SerIdSize for SerId {
        fn size(&self) -> usize {
            2
        }
    }

    /// A trait to retrieve a `SerId` from a buffer.
    pub trait SerIdBuf {
        /// Deserialises a `SerId` from this buffer.
        fn get_ser_id(&mut self) -> SerId;
    }

    /// A trait to put a `SerId` into a mutable buffer.
    pub trait SerIdBufMut {
        /// Serialises a `SerId` into this buffer.
        fn put_ser_id(&mut self, ser_id: SerId) -> ();
    }

    impl<B: Buf> SerIdBuf for B {
        fn get_ser_id(&mut self) -> SerId {
            self.get_u16()
        }
    }

    impl<B: BufMut> SerIdBufMut for B {
        fn put_ser_id(&mut self, ser_id: SerId) -> () {
            self.put_u16(ser_id)
        }
    }
}

#[cfg(feature = "ser_id_8")]
mod ser_id {
    use super::SerIdSize;
    use bytes::{Buf, BufMut};

    /// Type alias for the concrete implementation of serialisation ids.
    ///
    /// This version is activated by the `ser_id_8` feature flag, and uses a `u8` as the underlying size.
    pub type SerId = u8;

    impl SerIdSize for SerId {
        fn size(&self) -> usize {
            1
        }
    }

    /// A trait to retrieve a `SerId` from a buffer.
    pub trait SerIdBuf {
        /// Deserialises a `SerId` from this buffer.
        fn get_ser_id(&mut self) -> SerId;
    }

    /// A trait to put a `SerId` into a mutable buffer.
    pub trait SerIdBufMut {
        /// Serialises a `SerId` into this buffer.
        fn put_ser_id(&mut self, ser_id: SerId) -> ();
    }

    impl<B: Buf> SerIdBuf for B {
        fn get_ser_id(&mut self) -> SerId {
            self.get_u8()
        }
    }

    impl<B: BufMut> SerIdBufMut for B {
        fn put_ser_id(&mut self, ser_id: SerId) -> () {
            self.put_u8(ser_id)
        }
    }
}

pub use ser_id::*;

/// A fallable version of the `Clone` trait
pub trait TryClone: Sized {
    /// Tries to produce a copy of `self` or returns an error
    fn try_clone(&self) -> Result<Self, SerError>;
}

impl<T: Clone> TryClone for T {
    fn try_clone(&self) -> Result<Self, SerError> {
        Ok(self.clone())
    }
}

/// A module with helper functions for serialisation tests
pub mod ser_test_helpers {
    use super::*;

    /// Directly serialise something into the given buf
    ///
    /// This function panics if serialisation fails.
    pub fn just_serialise<S>(si: S, buf: &mut dyn BufMut) -> ()
    where
        S: Into<Box<dyn Serialisable>>,
    {
        let s: Box<dyn Serialisable> = si.into();
        s.serialise(buf).expect("Did not serialise correctly");
    }

    /// Directly serialise something into some buf and then throw it away
    ///
    /// This function panics if serialisation fails.
    pub fn test_serialise<S>(si: S) -> ()
    where
        S: Into<Box<dyn Serialisable>>,
    {
        let mut buf: Vec<u8> = Vec::new();
        just_serialise(si, &mut buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::BytesMut; //IntoBuf

    #[derive(PartialEq, Debug, Clone)]
    struct Test1 {
        i: u64,
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    struct T1Ser;

    impl Serialiser<Test1> for T1Ser {
        fn ser_id(&self) -> SerId {
            1
        }

        fn size_hint(&self) -> Option<usize> {
            Some(88)
        }

        fn serialise(&self, v: &Test1, buf: &mut dyn BufMut) -> Result<(), SerError> {
            buf.put_u64(v.i);
            for i in 0..10 {
                buf.put_u64(i);
            }
            Result::Ok(())
        }
    }

    impl Deserialiser<Test1> for T1Ser {
        const SER_ID: SerId = 1;

        fn deserialise(buf: &mut dyn Buf) -> Result<Test1, SerError> {
            let i = buf.get_u64();
            Result::Ok(Test1 { i })
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
        let mut buf = mbuf.freeze();
        let t1_res = T1Ser::deserialise(&mut buf);
        match t1_res {
            Ok(t2) => assert_eq!(t1c, t2),
            Err(e) => panic!("{}", e),
        }
    }
}

use bytes::{Buf, BufMut};
use std::{
    any::Any,
    fmt::{self, Debug},
};

use super::*;

mod core;
mod default_serialisers;
pub mod helpers;
#[cfg(feature = "protobuf")]
pub mod protobuf_serialisers;

pub use self::{core::*, default_serialisers::*};

#[cfg(feature = "ser_id_64")]
mod ser_id {
    use bytes::{Buf, BufMut};

    pub type SerId = u64;
    pub trait SerIdSize {
        fn size(&self) -> usize;
    }
    pub trait SerIdBuf {
        fn get_ser_id(&mut self) -> SerId;
    }
    pub trait SerIdBufMut {
        fn put_ser_id(&mut self, ser_id: SerId) -> ();
    }
    impl SerIdSize for SerId {
        fn size(&self) -> usize {
            8
        }
    }
    impl<B: Buf> SerIdBuf for B {
        fn get_ser_id(&mut self) -> SerId {
            self.get_u64_be()
        }
    }
    impl<B: BufMut> SerIdBufMut for B {
        fn put_ser_id(&mut self, ser_id: SerId) -> () {
            self.put_u64_be(ser_id)
        }
    }
}
pub use ser_id::*;

pub mod ser_test_helpers {
    use super::*;

    /// Directly serialise something into the given buf. Mostly for testing.
    pub fn just_serialise<S>(si: S, buf: &mut dyn BufMut) -> ()
    where
        S: Into<Box<dyn Serialisable>>,
    {
        let s: Box<dyn Serialisable> = si.into();
        s.serialise(buf).expect("Did not serialise correctly");
    }

    /// Directly serialise something into some buf and throw it away. Only for testing.
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
    use bytes::{BytesMut, IntoBuf};

    #[derive(PartialEq, Debug, Clone)]
    struct Test1 {
        i: u64,
    }

    struct T1Ser;
    impl Serialiser<Test1> for T1Ser {
        fn ser_id(&self) -> SerId {
            1
        }

        fn size_hint(&self) -> Option<usize> {
            Some(88)
        }

        fn serialise(&self, v: &Test1, buf: &mut dyn BufMut) -> Result<(), SerError> {
            buf.put_u64_be(v.i);
            for i in 0..10 {
                buf.put_u64_be(i);
            }
            Result::Ok(())
        }
    }
    impl Deserialiser<Test1> for T1Ser {
        const SER_ID: SerId = 1;

        fn deserialise(buf: &mut dyn Buf) -> Result<Test1, SerError> {
            let i = buf.get_u64_be();
            Result::Ok(Test1 { i })
        }
    }

    // #[test]
    // fn trivial_deser_equivalence() {
    //     let t1 = Test1 { i: 42 };
    //     let t1c = t1.clone();
    //     //let t1_des: Deserialisable<Test1> = t1;

    //     //assert_eq!(1, Deserialisable::<Test1>::id(&t1));

    //     match Deserialisable::<Test1>::get_deserialised(t1) {
    //         Ok(t2) => assert_eq!(t1c, t2),
    //         Err(e) => panic!(e),
    //     }

    //     let t1b = Box::new(t1c.clone());

    //     match Deserialisable::<Test1>::get_deserialised(t1b) {
    //         Ok(t2) => assert_eq!(t1c, t2),
    //         Err(e) => panic!(e),
    //     }
    // }

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

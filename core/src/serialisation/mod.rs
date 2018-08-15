use bytes::{Buf, BufMut};
use std::any::Any;
use std::fmt;
use std::fmt::Debug;

use super::*;

pub use self::core::*;

mod core;
pub mod helpers;

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
            buf.put_u64_be(v.i);
            for i in 0..10 {
                buf.put_u64_be(i);
            }
            Result::Ok(())
        }
    }
    impl Deserialiser<Test1> for T1Ser {
        fn deserialise(buf: &mut Buf) -> Result<Test1, SerError> {
            let i = buf.get_u64_be();
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

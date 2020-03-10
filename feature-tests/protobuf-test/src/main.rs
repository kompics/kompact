#![allow(unused_parens)]

//#[macro_use]
//extern crate component_definition_derive;

mod messages;

fn main() {
    unimplemented!();
}

#[cfg(test)]
mod tests {
    use super::messages::*;
    use bytes::BytesMut;

    use kompact::{prelude::*, protobuf_serialisers::*};
    use ser_test_helpers::{just_serialise, test_serialise};

    #[test]
    fn serialisation() {
        let ex = SearchRequest::new();
        test_serialise((ex, ProtobufSer {}));

        let ex2 = SearchResponse::new();
        test_serialise((ex2, ProtobufSer {}));
    }

    #[test]
    fn ser_deser() {
        {
            let ex = SearchRequest::new();
            let ex1 = ex.clone();
            let mut mbuf = BytesMut::with_capacity(64);
            just_serialise((ex, ProtobufSer {}), &mut mbuf);
            //let buf = mbuf.into_buf();
            let ds = ProtobufDeser {
                msg: SearchRequest::new(),
                buf: mbuf,
            }; //(ex2, buf.into_buf());
            let ex3: SearchRequest = ds.get_deserialised().expect("Did not deserialise");
            assert_eq!(ex1, ex3);
        }

        {
            let ex = SearchResponse::new();
            let ex1 = ex.clone();
            let mut mbuf = BytesMut::with_capacity(64);
            just_serialise((ex, ProtobufSer {}), &mut mbuf);
            //let buf = mbuf.into_buf();
            let ds = ProtobufDeser {
                msg: SearchResponse::new(),
                buf: mbuf,
            }; //(ex2, buf.into_buf());
            let ex3: SearchResponse = ds.get_deserialised().expect("Did not deserialise");
            assert_eq!(ex1, ex3);
        }
    }

    #[test]
    #[should_panic]
    fn ser_wrong_deser() {
        let ex = SearchRequest::new();
        let _ex1 = ex.clone();
        let mut mbuf = BytesMut::with_capacity(64);
        just_serialise((ex, ProtobufSer {}), &mut mbuf);
        //let buf = mbuf.into_buf();
        let ds = ProtobufDeser {
            msg: SearchResponse::new(),
            buf: mbuf,
        }; //(ex2, buf.into_buf());
        let _ex3: SearchResponse = ds.get_deserialised().expect("Did not deserialise");
        // this should fail!
        unreachable!();
    }
}

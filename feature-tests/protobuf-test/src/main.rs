#![allow(unused_parens)]
extern crate kompact;
extern crate protobuf;
extern crate bytes;
//#[macro_use]
//extern crate component_definition_derive;

mod messages;

fn main() {
    unimplemented!();
}

#[cfg(test)]
mod tests {
    use super::messages::*;
    use kompact::ser_test_helpers::*;
    use kompact::protobuf_serialisers::*;
    use kompact::prelude::*;
    use bytes::BytesMut;

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
            let mut buf = mbuf.into_buf();
            let ds = ProtobufDeser {msg: SearchRequest::new(), buf: buf}; //(ex2, buf.into_buf());
            let ex3: SearchRequest = ds.get_deserialised().expect("Did not deserialise");
            assert_eq!(ex1, ex3);
        }
        
        {
            let ex = SearchResponse::new();
            let ex1 = ex.clone();
            let mut mbuf = BytesMut::with_capacity(64);
            just_serialise((ex, ProtobufSer {}), &mut mbuf);
            let mut buf = mbuf.into_buf();
            let ds = ProtobufDeser {msg: SearchResponse::new(), buf: buf}; //(ex2, buf.into_buf());
            let ex3: SearchResponse = ds.get_deserialised().expect("Did not deserialise");
            assert_eq!(ex1, ex3);
        }
    }

    #[test]
    fn ser_wrong_deser() {
        let ex = SearchRequest::new();
        let ex1 = ex.clone();
        let mut mbuf = BytesMut::with_capacity(64);
        just_serialise((ex, ProtobufSer {}), &mut mbuf);
        let mut buf = mbuf.into_buf();
        let ds = ProtobufDeser {msg: SearchResponse::new(), buf: buf}; //(ex2, buf.into_buf());
        let ex3: SearchResponse = ds.get_deserialised().expect("Did not deserialise");
        // this should fail!
        unreachable!();
    }
}

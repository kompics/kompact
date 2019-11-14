//! Messaging types for sending and receiving messages between remote actors.

use crate::{
    actors::{ActorPath, DynActorRef, MessageBounds},
    net::events::NetworkEvent,
    serialisation::{Deserialiser, SerError, SerId, Serialisable, Serialiser},
    utils,
};
use bytes::{Bytes, IntoBuf};
use std::{any::Any, convert::TryFrom};
use uuid::Uuid;

pub mod framing;

#[derive(Debug)]
pub struct NetMessage {
    ser_id: SerId,
    sender: ActorPath,
    receiver: ActorPath,
    data: HeapOrSer,
}
#[derive(Debug)]
pub enum HeapOrSer {
    Boxed(Box<dyn Any + Send + 'static>),
    Serialised(bytes::Bytes),
}
impl NetMessage {
    pub fn with_box(
        ser_id: SerId,
        sender: ActorPath,
        receiver: ActorPath,
        data: Box<dyn Any + Send + 'static>,
    ) -> NetMessage {
        NetMessage {
            ser_id,
            sender,
            receiver,
            data: HeapOrSer::Boxed(data),
        }
    }

    pub fn with_bytes(
        ser_id: SerId,
        sender: ActorPath,
        receiver: ActorPath,
        data: Bytes,
    ) -> NetMessage {
        NetMessage {
            ser_id,
            sender,
            receiver,
            data: HeapOrSer::Serialised(data),
        }
    }

    /// Attempts to deserialise the contents into an instance of `T` using `D`,
    /// after first checking for a `ser_id` match with `D`.
    pub fn try_deserialise<T: 'static, D>(self) -> Result<T, UnpackError<Self>>
    where
        D: Deserialiser<T>,
    {
        if self.ser_id == D::SER_ID {
            self.try_deserialise_unchecked::<T, D>()
        } else {
            Err(UnpackError::NoIdMatch(self))
        }
    }

    /// Attempts to deserialise the contents into an instance of `T` using `D`,
    /// without checking the `ser_id` first for a match.
    ///
    /// Only use this, if you have already checked for a `ser_id` match!
    /// Otherwise use [try_deserialise](try_deserialise).
    pub fn try_deserialise_unchecked<T: 'static, D>(self) -> Result<T, UnpackError<Self>>
    where
        D: Deserialiser<T>,
    {
        let NetMessage {
            ser_id,
            sender,
            receiver,
            data,
        } = self;
        match data {
            HeapOrSer::Boxed(b) => b
                .downcast::<T>()
                .map(|b| *b)
                .map_err(|b| UnpackError::NoCast(Self::with_box(ser_id, sender, receiver, b))),
            HeapOrSer::Serialised(buf) => {
                D::deserialise(&mut buf.into_buf()).map_err(|e| UnpackError::DeserError(e))
            }
        }
    }

    pub fn ser_id(&self) -> &SerId {
        &self.ser_id
    }

    pub fn sender(&self) -> &ActorPath {
        &self.sender
    }

    pub fn receiver(&self) -> &ActorPath {
        &self.receiver
    }

    pub fn data(&self) -> &HeapOrSer {
        &self.data
    }
}

/// An error that is thrown when deserialisation of a [NetMessage](NetMessage) is attempted.
#[derive(Debug)]
pub enum UnpackError<T> {
    /// `ser_id` did not match the given [Deserialiser](crate::serialisation::Deserialiser).
    NoIdMatch(T),
    /// `Box<dyn Any>::downcast()` failed.
    NoCast(T),
    /// An error occurred during deserialisation of the buffer.
    ///
    /// This can not contain `T`, as the buffer may have been corrupted during the failed attempt.
    DeserError(SerError),
}

#[derive(Debug, PartialEq)]
pub enum RegistrationError {
    DuplicateEntry,
    Unsupported,
}

/// Envelope representing an actor registration event.
///
/// Used for registering an [ActorRef](actors::ActorRef) with an [ActorPath](actors::ActorPath).
#[derive(Debug)]
pub enum RegistrationEnvelope {
    Register(
        DynActorRef,
        PathResolvable,
        Option<utils::Promise<Result<ActorPath, RegistrationError>>>,
    ),
}

#[derive(Debug)]
pub struct Serialised {
    pub ser_id: SerId,
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
        crate::serialisation::helpers::serialiser_to_serialised(t.0, t.1)
    }
}
// Doesn't work due to: https://github.com/rust-lang/rust/issues/50133
// Use [serialise_to_serialised](crate::serialisation::helpers::serialise_to_serialised(ser))
// or the `dyn` version below instead.
// impl<S> TryFrom<&S> for Serialised
// where
//     S: Serialisable,
// {
//     type Error = SerError;

//     fn try_from(ser: &S) -> Result<Self, Self::Error> {
//         crate::serialisation::helpers::serialise_to_serialised(ser)
//     }
// }

impl TryFrom<&dyn Serialisable> for Serialised {
    type Error = SerError;

    fn try_from(ser: &dyn Serialisable) -> Result<Self, Self::Error> {
        crate::serialisation::helpers::serialise_to_serialised(ser)
    }
}

impl<E> TryFrom<Result<Serialised, E>> for Serialised {
    type Error = E;

    fn try_from(res: Result<Serialised, E>) -> Result<Self, Self::Error> {
        res
    }
}

#[derive(Debug)]
pub enum DispatchData {
    Lazy(Box<dyn Serialisable>),
    Eager(Serialised),
}
impl DispatchData {
    pub fn ser_id(&self) -> SerId {
        match self {
            DispatchData::Lazy(ser) => ser.ser_id(),
            DispatchData::Eager(ser) => ser.ser_id,
        }
    }

    pub fn to_local(self, src: ActorPath, dst: ActorPath) -> Result<NetMessage, SerError> {
        match self {
            DispatchData::Lazy(ser) => {
                let ser_id = ser.ser_id();
                match ser.local() {
                    Ok(s) => Ok(NetMessage::with_box(ser_id, src, dst, s)),
                    Err(ser) => crate::serialisation::helpers::serialise_to_msg(src, dst, ser),
                }
            }
            DispatchData::Eager(ser) => Ok(NetMessage::with_bytes(ser.ser_id, src, dst, ser.data)),
        }
    }

    pub fn to_serialised(self, src: ActorPath, dst: ActorPath) -> Result<Bytes, SerError> {
        match self {
            DispatchData::Lazy(ser) => {
                crate::serialisation::helpers::serialise_msg(&src, &dst, ser)
            }
            DispatchData::Eager(ser) => {
                crate::serialisation::helpers::embed_in_msg(&src, &dst, ser)
            }
        }
    }
}

/// Envelopes destined for the dispatcher
#[derive(Debug)]
pub enum DispatchEnvelope {
    Msg {
        src: PathResolvable,
        dst: ActorPath,
        msg: DispatchData,
    },
    Registration(RegistrationEnvelope),
    Event(EventEnvelope),
}

#[derive(Debug)]
pub enum EventEnvelope {
    Network(NetworkEvent),
}

#[derive(Debug)]
pub enum MsgEnvelope<M: MessageBounds> {
    Typed(M),
    Net(NetMessage),
}

#[derive(Debug, Clone)]
pub enum PathResolvable {
    Path(ActorPath),
    ActorId(Uuid),
    Alias(String),
    System,
}

// macro_rules! match_deser_stm {
//     ($msg:expr; $id:ident : $target_ty:ty => $rhs:expr) => {
//         &<$target_ty as Deserialiser<$target_ty>>::SER_ID => {
//             let $id = $msg.try_deserialise_unchecked::<$target_ty, $target_ty>().unwrap();
//             $rhs
//         }
//     };
//     ($msg:expr; $id:ident : $target_ty:ty [$deser:ty] => $rhs:expr) => {
//         &<$deser as Deserialiser<$target_ty>>::SER_ID => {
//             let $id = $msg.try_deserialise_unchecked::<$target_ty, $deser>().unwrap();
//             $rhs
//         }
//     };
// }

#[macro_export]
macro_rules! match_deser {
    ($msg:expr ; { $($id:ident : $target_ty:ty [$deser:ty] => $rhs:expr),* , }) => {
        match $msg.ser_id() {
            $( &<$deser as $crate::prelude::Deserialiser<$target_ty>>::SER_ID => {
            let $id = $msg.try_deserialise_unchecked::<$target_ty, $deser>().unwrap();
            $rhs
        } )*,
        _ => unimplemented!(),
        }
    };
    ($msg:expr ; { $($id:ident : $target_ty:ty [$deser:ty] => $rhs:expr),* , !Err($e:pat) => $err_handler:expr, }) => {
        match $msg.ser_id() {
            $( &<$deser as $crate::prelude::Deserialiser<$target_ty>>::SER_ID => {
            match $msg.try_deserialise_unchecked::<$target_ty, $deser>() {
                Ok($id) => $rhs,
                Err($e) => $err_handler,
            }
        } )*,
        _ => unimplemented!(),
        }
    };
    ($msg:expr ; { $($id:ident : $target_ty:ty [$deser:ty] => $rhs:expr),* , _ => $other:expr, }) => {
        match $msg.ser_id() {
            $( &<$deser as $crate::prelude::Deserialiser<$target_ty>>::SER_ID => {
            let $id = $msg.try_deserialise_unchecked::<$target_ty, $deser>().unwrap();
            $rhs
        } )*,
        _ => $other,
        }
    };
    ($msg:expr ; { $($id:ident : $target_ty:ty [$deser:ty] => $rhs:expr),* , !Err($e:pat) => $err_handler:expr , _ => $other:expr, }) => {
        match $msg.ser_id() {
            $( &<$deser as $crate::prelude::Deserialiser<$target_ty>>::SER_ID => {
            match $msg.try_deserialise_unchecked::<$target_ty, $deser>() {
                Ok($id) => $rhs,
                Err($e) => $err_handler,
            }
        } )*,
        _ => $other,
        }
    };
}

#[cfg(test)]
mod deser_macro_tests {
    use super::*;
    use crate::serialisation::Serialiser;
    use bytes::{Buf, BufMut};
    use std::str::FromStr;

    #[test]
    fn simple_macro_test() {
        simple_macro_test_impl(|msg| {
            match_deser! { msg; {
                    res: MsgA [MsgA] => EitherAOrB::A(res),
                    res: MsgB [BSer] => EitherAOrB::B(res),
                }
            }
        })
    }

    #[test]
    fn simple_macro_test_with_other() {
        simple_macro_test_impl(|msg| {
            match_deser! { msg; {
                    res: MsgA [MsgA] => EitherAOrB::A(res),
                    res: MsgB [BSer] => EitherAOrB::B(res),
                    _ => unimplemented!("Should be either MsgA or MsgB!"),
                }
            }
        })
    }

    #[test]
    #[should_panic(expected = "test panic please ignore")]
    fn simple_macro_test_with_err() {
        simple_macro_test_impl(|msg| {
            match_deser! { msg; {
                    res: MsgA [MsgA] => EitherAOrB::A(res),
                    res: MsgB [BSer] => EitherAOrB::B(res),
                    !Err(_e) => panic!("test panic please ignore"),
                }
            }
        });
        simple_macro_test_err_impl(|msg| {
            match_deser! { msg; {
                    res: MsgA [MsgA] => EitherAOrB::A(res),
                    res: MsgB [BSer] => EitherAOrB::B(res),
                    !Err(_e) => panic!("test panic please ignore"),
                }
            }
        });
    }

    #[test]
    #[should_panic(expected = "test panic please ignore")]
    fn simple_macro_test_with_err_and_other() {
        simple_macro_test_impl(|msg| {
            match_deser! { msg; {
                    res: MsgA [MsgA] => EitherAOrB::A(res),
                    res: MsgB [BSer] => EitherAOrB::B(res),
                    !Err(_e) => panic!("test panic please ignore"),
                    _ => unimplemented!("Should be either MsgA or MsgB!"),
                }
            }
        });
        // alternative design
        //  simple_macro_test_impl(|msg| {
        //     match_deser! { msg; {
        //             Msg(res: MsgA [MsgA]) => EitherAOrB::A(res),
        //             Msg(res: MsgB [BSer]) => EitherAOrB::B(res),
        //             Err(_e) => panic!("test panic please ignore"),
        //             _ => unimplemented!("Should be either MsgA or MsgB!"),
        //         }
        //     }
        // });
        simple_macro_test_err_impl(|msg| {
            match_deser! { msg; {
                    res: MsgA [MsgA] => EitherAOrB::A(res),
                    res: MsgB [BSer] => EitherAOrB::B(res),
                    !Err(_e) => panic!("test panic please ignore"),
                    _ => unimplemented!("Should be either MsgA or MsgB!"),
                }
            }
        });
    }

    #[test]
    fn simple_no_macro_test() {
        simple_macro_test_impl(|msg| match msg.ser_id() {
            &MsgA::SER_ID => {
                let res = msg
                    .try_deserialise_unchecked::<MsgA, MsgA>()
                    .expect("MsgA should deserialise!");
                EitherAOrB::A(res)
            }
            &BSer::SER_ID => {
                let res = msg
                    .try_deserialise_unchecked::<MsgB, BSer>()
                    .expect("MsgB should deserialise!");
                EitherAOrB::B(res)
            }
            _ => unimplemented!("Should be either MsgA or MsgB!"),
        })
    }

    fn simple_macro_test_impl<F>(f: F)
    where
        F: Fn(NetMessage) -> EitherAOrB,
    {
        let ap = ActorPath::from_str("local://127.0.0.1:12345/testme").expect("an ActorPath");

        let msg_a = MsgA::new(54);
        let msg_b = MsgB::new(true);

        let msg_a_ser = crate::serialisation::helpers::serialise_to_msg(
            ap.clone(),
            ap.clone(),
            Box::new(msg_a),
        )
        .expect("MsgA should serialise!");
        let msg_b_ser = crate::serialisation::helpers::serialise_to_msg(
            ap.clone(),
            ap.clone(),
            (msg_b, BSer).into(),
        )
        .expect("MsgB should serialise!");

        assert!(f(msg_a_ser).is_a());
        assert!(f(msg_b_ser).is_b());
    }

    fn simple_macro_test_err_impl<F>(f: F)
    where
        F: Fn(NetMessage) -> EitherAOrB,
    {
        let ap = ActorPath::from_str("local://127.0.0.1:12345/testme").expect("an ActorPath");

        let msg = NetMessage::with_bytes(MsgA::SERID, ap.clone(), ap.clone(), Bytes::default());

        f(msg);
    }

    enum EitherAOrB {
        A(MsgA),
        B(MsgB),
    }
    impl EitherAOrB {
        fn is_a(&self) -> bool {
            match self {
                EitherAOrB::A(_) => true,
                EitherAOrB::B(_) => false,
            }
        }

        fn is_b(&self) -> bool {
            !self.is_a()
        }
    }

    #[derive(Debug)]
    struct MsgA {
        index: u64,
    }
    impl MsgA {
        const SERID: SerId = 42;

        fn new(index: u64) -> MsgA {
            MsgA { index }
        }
    }
    impl Serialisable for MsgA {
        fn ser_id(&self) -> SerId {
            Self::SERID
        }

        fn size_hint(&self) -> Option<usize> {
            Some(8)
        }

        fn serialise(&self, buf: &mut dyn BufMut) -> Result<(), SerError> {
            buf.put_u64_be(self.index);
            Ok(())
        }

        fn local(self: Box<Self>) -> Result<Box<dyn Any + Send>, Box<dyn Serialisable>> {
            Ok(self)
        }
    }
    impl Deserialiser<MsgA> for MsgA {
        const SER_ID: SerId = Self::SERID;

        fn deserialise(buf: &mut dyn Buf) -> Result<MsgA, SerError> {
            if buf.remaining() < 8 {
                return Err(SerError::InvalidData(
                    "Less than 8bytes remaining in buffer!".to_string(),
                ));
            }
            let index = buf.get_u64_be();
            let msg = MsgA { index };
            Ok(msg)
        }
    }

    #[derive(Debug)]
    struct MsgB {
        flag: bool,
    }
    impl MsgB {
        const SERID: SerId = 43;

        fn new(flag: bool) -> MsgB {
            MsgB { flag }
        }
    }
    #[derive(Clone)]
    struct BSer;
    impl Serialiser<MsgB> for BSer {
        fn ser_id(&self) -> SerId {
            MsgB::SERID
        }

        fn size_hint(&self) -> Option<usize> {
            Some(1)
        }

        fn serialise(&self, v: &MsgB, buf: &mut dyn BufMut) -> Result<(), SerError> {
            let num = if v.flag { 1u8 } else { 0u8 };
            buf.put_u8(num);
            Ok(())
        }
    }
    impl Deserialiser<MsgB> for BSer {
        const SER_ID: SerId = MsgB::SERID;

        fn deserialise(buf: &mut dyn Buf) -> Result<MsgB, SerError> {
            let num = buf.get_u8();
            let flag = num == 1u8;
            let msg = MsgB { flag };
            Ok(msg)
        }
    }
}

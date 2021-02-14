/// A macro to make matching serialisation ids and deserialising easier
///
/// This macro basically generates a large match statement on the serialisation ids
/// and then uses [try_deserialise_unchecked](crate::prelude::NetMessage::try_deserialise_unchecked)
/// to get a value for the matched type, which it then uses to invoke the right-hand side
/// which expects that particular type.
///
/// # Basic Example
///
/// ```
/// use kompact::prelude::*;
/// # use kompact::doctest_helpers;
/// use bytes::BytesMut;
///
/// # let some_path: ActorPath = doctest_helpers::TEST_PATH.parse().expect("actor path");
/// # let some_path2 = some_path.clone();
///
/// let test_str = "Test me".to_string();
/// // serialise the string
/// let mut mbuf = BytesMut::with_capacity(test_str.size_hint().expect("size hint"));
/// test_str.serialise(&mut mbuf).expect("serialise");
/// // create a net message
/// let buf = mbuf.freeze();
/// let msg = NetMessage::with_bytes(String::SER_ID, some_path, some_path2, buf);
/// // try to deserialise it again
/// match_deser!{
///     msg {
///         msg(_num): u64                       => unreachable!("It's definitely not a u64..."),
///         msg(test_res): String [using String] => assert_eq!(test_str, test_res),
///     }
/// }
/// ```
///
/// You can specify the [Deserialiser](crate::prelude::Deserialiser) to use as `D` via `[using D]`
/// (as is done above for `String`).
/// If no [Deserialiser](crate::prelude::Deserialiser) is specified the macro will try to use
/// the target message type as a [Deserialiser](crate::prelude::Deserialiser).
///
/// # Example with Error Handling
///
/// ```
/// use kompact::prelude::*;
/// # use kompact::doctest_helpers;
/// use bytes::BytesMut;
///
/// # let some_path: ActorPath = doctest_helpers::TEST_PATH.parse().expect("actor path");
/// # let some_path2 = some_path.clone();
///
/// let test_str = "Test me".to_string();
/// // serialise the string
/// let mut mbuf = BytesMut::with_capacity(test_str.size_hint().expect("size hint"));
/// test_str.serialise(&mut mbuf).expect("serialise");
/// // create a net message
/// let buf = mbuf.freeze();
/// let msg = NetMessage::with_bytes(String::SER_ID, some_path, some_path2, buf);
/// // try to deserialise it again
/// match_deser! {
///     msg {
///         msg(_num): u64                       => unreachable!("It's definitely not a u64..."),
///         msg(test_res): String [using String] => assert_eq!(test_str, test_res),
///         err(error)                           => panic!("Some error occurred during deserialisation: {:?}", error),
///         default(_)                           => unreachable!("It's definitely not...whatever this is..."),
///     }
/// }
/// ```
/// # Note
///
/// You can use an expression instead of a simple identifier for the message to be matched,
/// but it must be wrapped into parentheses in that case to not confuse the Rust compiler.
#[macro_export]
macro_rules! match_deser {
    ($msg:ident {$($tokens:tt)*}) => {{
        let msg = $msg;
        $crate::match_deser_internal!(
            msg;
            $($tokens)*
        )
    }};
    (($msg:expr) {$($tokens:tt)*}) => {{
        let msg = $msg;
        $crate::match_deser_internal!(
            msg;
            $($tokens)*
        )
    }};
}

#[macro_export]
#[doc(hidden)]
macro_rules! match_deser_internal {
    // The list is empty. Now check the arguments of each processed case.
    (@list
        $msg:ident;
        ();
        $cases:tt
    ) => {
        $crate::match_deser_internal!(
            @case
            $msg;
            $cases;
            ();
            ();
            ()
        )
    };
    // Separate by comma
    (@list
        $msg:ident;
        ($case:ident ($($args:tt)*) $(: $msg_ty:ty)? $([using $deser_ty:ty])? => $body:expr, $($tail:tt)*);
        ($($head:tt)*)
        ) => {
            $crate::match_deser_internal!(
            @list
            $msg;
            ($($tail)*);
            ($($head)* $case ($($args)*) $(: $msg_ty)* $([$deser_ty])* => { $body },)
        )
    };
    // Don't require a comma if it has a proper block.
    (@list
        $msg:ident;
        ($case:ident ($($args:tt)*) $(: $msg_ty:ty)? $([using $deser_ty:ty])? => $body:block, $($tail:tt)*);
        ($($head:tt)*)
        ) => {
            $crate::match_deser_internal!(
            @list
            $msg;
            ($($tail)*);
            ($($head)* $case ($($args)*) $(: $msg_ty)* $([$deser_ty])* => { $body },)
        )
    };
    // Last case
    (@list
        $msg:ident;
        ($case:ident ($($args:tt)*) $(: $msg_ty:ty)? $([using $deser_ty:ty])? => $body:expr);
        ($($head:tt)*)
        ) => {
            $crate::match_deser_internal!(
            @list
            $msg;
            ();
            ($($head)* $case ($($args)*) $(: $msg_ty)* $([$deser_ty])* => { $body },)
        )
    };
    // Accept trailing comma
    (@list
        $msg:ident;
        ($case:ident ($($args:tt)*) $(: $msg_ty:ty)? $([using $deser_ty:ty])? => $body:expr,);
        ($($head:tt)*)
        ) => {
            $crate::match_deser_internal!(
            @list
            $msg;
            ();
            ($($head)* $case ($($args)*) $(: $msg_ty)* $([$deser_ty])* => { $body },)
        )
    };
    // Inject default and error case
    (@case
        $msg:ident;
        ();
        $msgs:tt;
        ();
        ()
    ) => {
        $crate::match_deser_internal!(
            @init
            $msg;
            $msgs;
            (err(e) => { panic!("{:?}", e); });
            (default(_) => { unimplemented!(); })
        )
    };
    // Inject default case
    (@case
        $msg:ident;
        ();
        $msgs:tt;
        $errors:tt;
        ()
    ) => {
        $crate::match_deser_internal!(
            @init
            $msg;
            $msgs;
            $errors;
            (default(_) => { unimplemented!(); })
        )
    };
    // Inject error handler
    (@case
        $msg:ident;
        ();
        $msgs:tt;
        ();
        $default:tt
    ) => {
        $crate::match_deser_internal!(
            @init
            $msg;
            $msgs;
            (err(e) => { panic!("{:?}", e); });
            $default
        )
    };
    // Success! All cases were parsed.
    (@case
        $msg:ident;
        ();
        $msgs:tt;
        $error:tt;
        $default:tt
    ) => {
        $crate::match_deser_internal!(
            @init
            $msg;
            $msgs;
            $error;
            $default
        )
    };
    // Check the format of a msg case where msg_ty: Deserialiser<msg_ty>.
    (@case
        $msg:ident;
        (msg($msg_pat:pat) : $msg_ty:ty => $body:tt, $($tail:tt)*);
        ($($msgs:tt)*);
        $error:tt;
        $default:tt
    ) => {
        $crate::match_deser_internal!(
            @case
            $msg;
            ($($tail)*);
            ($($msgs)* msg($msg_pat) : $msg_ty [$msg_ty] => $body,);
            $error;
            $default
        )
    };
    // Check the format of a msg case where msg_ty != deser_ty.
    (@case
        $msg:ident;
        (msg($msg_pat:pat) : $msg_ty:ty [$deser_ty:ty] => $body:tt, $($tail:tt)*);
        ($($msgs:tt)*);
        $error:tt;
        $default:tt
    ) => {
        $crate::match_deser_internal!(
            @case
            $msg;
            ($($tail)*);
            ($($msgs)* msg($msg_pat) : $msg_ty [$deser_ty] => $body,);
            $error;
            $default
        )
    };
    // Check the format of an err case
    (@case
        $msg:ident;
        (err($err_pat:pat) => $body:tt, $($tail:tt)*);
        $msgs:tt;
        ();
        $default:tt
    ) => {
        $crate::match_deser_internal!(
            @case
            $msg;
            ($($tail)*);
            $msgs;
            (err($err_pat) => $body);
            $default
        )
    };
    // Can only have one err case!
    (@case
        $msg:ident;
        (err($err_pat:pat) => $body:tt, $($tail:tt)*);
        $msgs:tt;
        $error:tt;
        $default:tt
    ) => {
        compile_error!("Only a single `err(_)` arm is allowed in `match_deser!`");
    };
    // Check the format of a default case
    (@case
        $msg:ident;
        (default(_) => $body:tt, $($tail:tt)*);
        $msgs:tt;
        $error:tt;
        ()
    ) => {
        $crate::match_deser_internal!(
            @case
            $msg;
            ($($tail)*);
            $msgs;
            $error;
            (default(_) => $body)
        )
    };
    // Can only have one default case!
    (@case
        $msg:ident;
        (default(_) => $body:tt, $($tail:tt)*);
        $msgs:tt;
        $error:tt;
        $default:tt
    ) => {
        compile_error!("Only a single `default(_)` arm is allowed in `match_deser!`");
    };
    (@init
        $msg:ident;
        ($(msg($msg_pat:pat) : $msg_ty:ty [$deser_ty:ty] => $body:tt,)*);
        (err($err_pat:pat) => $err_body:tt);
        (default(_) => $default_body:tt)
        ) => {
        match $msg.ser_id() {
            $( &<$deser_ty as $crate::prelude::Deserialiser<$msg_ty>>::SER_ID => {
                match $msg.try_deserialise_unchecked::<$msg_ty, $deser_ty>() {
                    Ok($msg_pat) => $body,
                    Err($err_pat) => $err_body,
                }
            } ),*
            _ => $default_body,
        }
    };
    ($msg:ident;) => {
        compile_error!("Empty `match_deser!` block");
    };
    ($msg:ident; $($tokens:tt)*) => {
        $crate::match_deser_internal!(
            @list
            $msg;
            ($($tokens)*);
            ()
        )
    };
}

#[cfg(test)]
mod deser_macro_tests {
    use crate::{
        messaging::*,
        serialisation::{Deserialiser, Serialisable, Serialiser},
    };
    use bytes::{Buf, BufMut};
    use std::str::FromStr;

    // #[test]
    // fn new_macro_syntax_test() {
    //     simple_macro_test_impl(|msg| {
    //         //trace_macros!(true);
    //         let res = new_match_deser! { msg {
    //              msg(res): MsgA => EitherAOrB::A(res),
    //              msg(res): MsgB [using BSer] => EitherAOrB::B(res),
    //              err(_e) => panic!("test panic please ignore"),
    //              default(_) => unimplemented!("Should be either MsgA or MsgB!"),
    //             }
    //         };
    //         //trace_macros!(false);
    //         res
    //     })
    // }

    #[test]
    fn simple_macro_test() {
        simple_macro_test_impl(|msg| {
            match_deser! {
                msg {
                    msg(res): MsgA => EitherAOrB::A(res),
                    msg(res): MsgB [using BSer] => EitherAOrB::B(res),
                }
            }
        })
    }

    #[test]
    fn simple_macro_test_with_other() {
        simple_macro_test_impl(|msg| {
            match_deser! {
                msg {
                    msg(res): MsgA  => EitherAOrB::A(res),
                    msg(res): MsgB [using BSer] => EitherAOrB::B(res),
                    default(_) => unimplemented!("Should be either MsgA or MsgB!"),
                }
            }
        })
    }

    #[test]
    #[should_panic(expected = "test panic please ignore")]
    fn simple_macro_test_with_err() {
        simple_macro_test_impl(|msg| {
            match_deser! {
                msg {
                    msg(res): MsgA => EitherAOrB::A(res),
                    msg(res): MsgB [using BSer] => EitherAOrB::B(res),
                    err(_e) => panic!("test panic please ignore"),
                }
            }
        });
        simple_macro_test_err_impl(|msg| {
            match_deser! {
                msg {
                    msg(res): MsgA => EitherAOrB::A(res),
                    msg(res): MsgB [using BSer] => EitherAOrB::B(res),
                    err(_e) => panic!("test panic please ignore"),
                }
            }
        });
    }

    #[test]
    #[should_panic(expected = "test panic please ignore")]
    fn simple_macro_test_with_err_and_other() {
        simple_macro_test_impl(|msg| {
            match_deser! {
                msg {
                    msg(res): MsgA => EitherAOrB::A(res),
                    msg(res): MsgB [using BSer] => EitherAOrB::B(res),
                    err(_e) => panic!("test panic please ignore"),
                    default(_) => unimplemented!("Should be either MsgA or MsgB!"),
                }
            }
        });
        simple_macro_test_err_impl(|msg| {
            match_deser! {
                msg {
                    msg(res): MsgA => EitherAOrB::A(res),
                    msg(res): MsgB [using BSer] => EitherAOrB::B(res),
                    err(_e) => panic!("test panic please ignore"),
                    default(_) => unimplemented!("Should be either MsgA or MsgB!"),
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

        let msg_a_ser = crate::serialisation::ser_helpers::serialise_to_msg(
            ap.clone(),
            ap.clone(),
            Box::new(msg_a),
        )
        .expect("MsgA should serialise!");
        let msg_b_ser = crate::serialisation::ser_helpers::serialise_to_msg(
            ap.clone(),
            ap,
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

        let msg = NetMessage::with_bytes(MsgA::SERID, ap.clone(), ap, Bytes::default());

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
            buf.put_u64(self.index);
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
            let index = buf.get_u64();
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

    #[derive(Clone, Copy, PartialEq, Eq)]
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

#![feature(test)]
extern crate test;

pub mod actorrefs;
pub mod hashes;

use std::any::*;
use std::sync::Arc;

pub enum Message {
    StaticRef(&'static dyn Any),
    Owned(Box<dyn Any>),
    Shared(Arc<dyn Any>),
}

pub fn do_with_message<F>(f: F, m: Message) -> u64
where
    F: Fn(&dyn Any) -> u64 + 'static,
{
    match m {
        Message::StaticRef(r) => f(r),
        Message::Owned(b) => f(b.as_ref()),
        Message::Shared(s) => f(s.as_ref()),
    }
}

pub struct IntBox(u64);

pub fn do_with_any_box(a: Box<dyn Any>) -> u64 {
    if let Some(ref ib) = a.downcast_ref::<IntBox>() {
        ib.0
    } else {
        unimplemented!();
    }
}

pub fn do_with_any_box_ref(a: &Box<dyn Any>) -> u64 {
    if let Some(ref ib) = a.downcast_ref::<IntBox>() {
        ib.0
    } else {
        unimplemented!();
    }
}

pub fn do_with_any_ref(a: &dyn Any) -> u64 {
    if let Some(ref ib) = a.downcast_ref::<IntBox>() {
        ib.0
    } else {
        unimplemented!();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test::Bencher;

    #[bench]
    fn bench_any_box(b: &mut Bencher) {
        b.iter(|| {
            let a: Box<dyn Any> = Box::new(IntBox(12345678)) as Box<dyn Any>;
            do_with_any_box(a);
        });
    }

    #[bench]
    fn bench_any_box_ref(b: &mut Bencher) {
        let a: Box<dyn Any> = Box::new(IntBox(12345678)) as Box<dyn Any>;
        b.iter(|| {
            do_with_any_box_ref(&a);
        });
    }

    #[bench]
    fn bench_any_ref_from_box(b: &mut Bencher) {
        let a: Box<dyn Any> = Box::new(IntBox(12345678)) as Box<dyn Any>;
        b.iter(|| {
            do_with_any_ref(a.as_ref());
        });
    }

    #[bench]
    fn bench_any_ref(b: &mut Bencher) {
        b.iter(|| {
            let a = IntBox(12345678);
            do_with_any_ref(&a as &dyn Any);
        });
    }

    const TEST_BOX: IntBox = IntBox(12345678);

    #[bench]
    fn bench_msg_with_any_ref(b: &mut Bencher) {
        b.iter(|| {
            let msg = Message::StaticRef(&TEST_BOX);
            do_with_message(do_with_any_ref, msg);
        });
    }

    #[bench]
    fn bench_msg_with_any_box(b: &mut Bencher) {
        b.iter(|| {
            let msg = Message::Owned(Box::new(TEST_BOX));
            do_with_message(do_with_any_ref, msg);
        });
    }

    #[bench]
    fn bench_msg_with_shared_any(b: &mut Bencher) {
        let test = Arc::new(TEST_BOX);
        b.iter(|| {
            let msg = Message::Shared(test.clone());
            do_with_message(do_with_any_ref, msg);
        });
    }
}

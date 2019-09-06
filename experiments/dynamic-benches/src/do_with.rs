use std::any::*;
use std::sync::Arc;
use criterion::criterion_group;
use criterion::criterion_main;
use criterion::{black_box, Criterion};

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


pub fn do_benches(c: &mut Criterion) {
    let mut g = c.benchmark_group("Do_With Benches");
    g.bench_function("bench Box<dyn Any>", |b| tests::bench_any_box(b));
    g.bench_function("bench &Box<dyn Any>", |b| tests::bench_any_box_ref(b));
    g.bench_function("bench Box<dyn Any>.as_ref()", |b| tests::bench_any_ref_from_box(b));
    g.bench_function("bench &dyn Any", |b| tests::bench_any_ref(b));
    g.bench_function("bench Message &'static Any", |b| tests::bench_msg_with_any_ref(b));
    g.bench_function("bench Message Box<dyn Any>", |b| tests::bench_msg_with_any_box(b));
    g.bench_function("bench Message Arc<dyn Any>", |b| tests::bench_msg_with_shared_any(b));
    g.finish();
}

mod tests {
    use super::*;
    use criterion::Bencher;

    
    pub fn bench_any_box(b: &mut Bencher) {
        b.iter(|| {
            let a: Box<dyn Any> = Box::new(IntBox(12345678)) as Box<dyn Any>;
            do_with_any_box(a);
        });
    }

    
    pub fn bench_any_box_ref(b: &mut Bencher) {
        let a: Box<dyn Any> = Box::new(IntBox(12345678)) as Box<dyn Any>;
        b.iter(|| {
            do_with_any_box_ref(&a);
        });
    }

    
    pub fn bench_any_ref_from_box(b: &mut Bencher) {
        let a: Box<dyn Any> = Box::new(IntBox(12345678)) as Box<dyn Any>;
        b.iter(|| {
            do_with_any_ref(a.as_ref());
        });
    }

    
    pub fn bench_any_ref(b: &mut Bencher) {
        b.iter(|| {
            let a = IntBox(12345678);
            do_with_any_ref(&a as &dyn Any);
        });
    }

    const TEST_BOX: IntBox = IntBox(12345678);

    
    pub fn bench_msg_with_any_ref(b: &mut Bencher) {
        b.iter(|| {
            let msg = Message::StaticRef(&TEST_BOX);
            do_with_message(do_with_any_ref, msg);
        });
    }

    
    pub fn bench_msg_with_any_box(b: &mut Bencher) {
        b.iter(|| {
            let msg = Message::Owned(Box::new(TEST_BOX));
            do_with_message(do_with_any_ref, msg);
        });
    }

    
    pub fn bench_msg_with_shared_any(b: &mut Bencher) {
        let test = Arc::new(TEST_BOX);
        b.iter(|| {
            let msg = Message::Shared(test.clone());
            do_with_message(do_with_any_ref, msg);
        });
    }
}

criterion_group!(do_with_benches, do_benches);
criterion_main!(do_with_benches);

use criterion::criterion_group;
use criterion::criterion_main;
use criterion::{black_box, Criterion};

fn fibonacci(n: u32) -> u32 {
    n * n * n * n * n
}

pub fn kompact_network_latency(c: &mut Criterion) {
    c.bench_function("fib 20", |b| b.iter(|| fibonacci(black_box(20))));
}

criterion_group!(latency_benches, kompact_network_latency);
criterion_main!(latency_benches);

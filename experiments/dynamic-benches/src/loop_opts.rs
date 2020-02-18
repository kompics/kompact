use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};

const DATA_SIZES: [usize; 4] = [1, 2, 10, 100];

pub fn loop_benches(c: &mut Criterion) {
    final_clone_bench(c);
}

fn final_clone_bench(c: &mut Criterion) {
    let mut g = c.benchmark_group("Final Clone Optimisation");
    for data_size in DATA_SIZES.iter() {
        g.bench_with_input(
            BenchmarkId::new("for loop", data_size),
            data_size,
            |b, &size| tests::bench_clone_for_loop(b, size),
        );
        g.bench_with_input(
            BenchmarkId::new("for_each", data_size),
            data_size,
            |b, &size| tests::bench_clone_for_each(b, size),
        );
        g.bench_with_input(
            BenchmarkId::new("while", data_size),
            data_size,
            |b, &size| tests::bench_clone_while(b, size),
        );
        g.bench_with_input(
            BenchmarkId::new("for_each_with", data_size),
            data_size,
            |b, &size| tests::bench_clone_while(b, size),
        );
    }
    g.finish();
}

#[derive(Clone)]
pub struct ExpensiveClone {
    data: Vec<u64>,
}
impl ExpensiveClone {
    pub fn medium() -> ExpensiveClone {
        let data: Vec<u64> = (0u64..1024u64).collect();
        ExpensiveClone { data }
    }

    pub fn consume(self, data: usize) {
        let consumable = (self, data);
        let _ = black_box(consumable);
    }
}

mod tests {
    use super::*;
    use criterion::{BatchSize, Bencher};
    use kompact::prelude::IterExtras;

    pub fn bench_clone_for_loop(b: &mut Bencher, data_size: usize) {
        let data: Vec<usize> = (0usize..data_size).collect();
        b.iter_batched(
            || ExpensiveClone::medium(),
            |my_item| {
                for i in data.iter() {
                    my_item.clone().consume(*i);
                }
            },
            BatchSize::SmallInput,
        );
    }

    pub fn bench_clone_for_each(b: &mut Bencher, data_size: usize) {
        let data: Vec<usize> = (0usize..data_size).collect();
        b.iter_batched(
            || ExpensiveClone::medium(),
            |my_item| {
                data.iter().for_each(|i| {
                    my_item.clone().consume(*i);
                });
            },
            BatchSize::SmallInput,
        );
    }

    pub fn bench_clone_while(b: &mut Bencher, data_size: usize) {
        let data: Vec<usize> = (0usize..data_size).collect();
        b.iter_batched(
            || ExpensiveClone::medium(),
            |my_item| {
                let mut it = data.iter();
                let mut current: Option<&usize> = it.next();
                let mut next: Option<&usize> = it.next();
                while next.is_some() {
                    let i = *current.take().unwrap();
                    my_item.clone().consume(i);
                    current = next;
                    next = it.next();
                }
                let i = *current.take().unwrap();
                my_item.consume(i);
            },
            BatchSize::SmallInput,
        );
    }

    pub fn bench_clone_for_each_with(b: &mut Bencher, data_size: usize) {
        let data: Vec<usize> = (0usize..data_size).collect();
        b.iter_batched(
            || ExpensiveClone::medium(),
            |my_item| {
                data.iter().for_each_with(my_item, |i, item| {
                    item.consume(*i);
                });
            },
            BatchSize::SmallInput,
        );
    }
}

criterion_group!(loop_opts_benches, loop_benches);
criterion_main!(loop_opts_benches);

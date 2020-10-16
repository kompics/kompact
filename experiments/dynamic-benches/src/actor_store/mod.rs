use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use kompact::{
    lookup::{ActorLookup, ActorStore},
    prelude::*,
};
use rand::prelude::*;
use std::sync::Arc;

mod sequence_store;

const DATA_SIZES: [usize; 4] = [1, 7, 100, 10000];
const MAX_DEPTH: usize = 5;
const NAME_PREFIX: &str = "node";
const ROUTER_PREFIX: &str = "route-me";

pub fn insert_benches(c: &mut Criterion) {
    let mut g = c.benchmark_group("Inserts");
    for data_size in DATA_SIZES.iter() {
        g.throughput(Throughput::Elements(*data_size as u64));
        g.bench_with_input(
            BenchmarkId::new("SequenceTrie", data_size),
            data_size,
            |b, &size| tests::bench_insert(b, sequence_store::ActorStore::new, size),
        );
        g.bench_with_input(
            BenchmarkId::new("PathTrie", data_size),
            data_size,
            |b, &size| tests::bench_insert(b, ActorStore::new, size),
        );
    }
    g.finish();
}

pub fn lookup_benches(c: &mut Criterion) {
    let mut g = c.benchmark_group("Lookups");
    for data_size in DATA_SIZES.iter() {
        g.throughput(Throughput::Elements(*data_size as u64));
        g.bench_with_input(
            BenchmarkId::new("SequenceTrie", data_size),
            data_size,
            |b, &size| tests::bench_lookup(b, sequence_store::ActorStore::new(), size),
        );
        g.bench_with_input(
            BenchmarkId::new("PathTrie", data_size),
            data_size,
            |b, &size| tests::bench_lookup(b, ActorStore::new(), size),
        );
    }
    g.finish();
}

pub fn group_lookup_benches(c: &mut Criterion) {
    let mut g = c.benchmark_group("GroupLookups");
    for data_size in DATA_SIZES.iter() {
        g.throughput(Throughput::Elements(1));
        g.bench_with_input(
            BenchmarkId::new("SequenceTrie", data_size),
            data_size,
            |b, &size| tests::bench_group_lookup(b, sequence_store::ActorStore::new(), size),
        );
        g.bench_with_input(
            BenchmarkId::new("PathTrie", data_size),
            data_size,
            |b, &size| tests::bench_group_lookup(b, ActorStore::new(), size),
        );
    }
    g.finish();
}

pub fn cleanup_benches(c: &mut Criterion) {
    let mut g = c.benchmark_group("Cleanup");
    for data_size in DATA_SIZES.iter() {
        g.bench_with_input(
            BenchmarkId::new("SequenceTrie", data_size),
            data_size,
            |b, &size| tests::bench_cleanup(b, sequence_store::ActorStore::new, size),
        );
        g.bench_with_input(
            BenchmarkId::new("PathTrie", data_size),
            data_size,
            |b, &size| tests::bench_cleanup(b, ActorStore::new, size),
        );
    }
    g.finish();
}

#[derive(ComponentDefinition, Actor)]
struct NopActor {
    ctx: ComponentContext<Self>,
}
ignore_lifecycle!(NopActor);
impl Default for NopActor {
    fn default() -> Self {
        NopActor {
            ctx: ComponentContext::uninitialised(),
        }
    }
}

struct DataSet {
    system: KompactSystem,
    actors: Vec<Arc<Component<NopActor>>>,
    paths: Vec<(PathResolvable, DynActorRef)>,
}

fn load_data(data_size: usize) -> DataSet {
    let system = KompactConfig::default().build().expect("system");
    let num_actors = 1.max(data_size / 3);
    let actors: Vec<Arc<Component<NopActor>>> = (0..num_actors)
        .map(|_| system.create(NopActor::default))
        .collect();
    let mut rng = rand::thread_rng();
    let mut paths: Vec<(PathResolvable, DynActorRef)> = Vec::with_capacity(data_size);
    for i in 0..data_size {
        let path_len = rng.gen_range(1, MAX_DEPTH);
        let path: Vec<String> = (1..=path_len)
            .map(|depth| {
                let level_width = 1.max(data_size / (depth * 2));
                let id = i % level_width;
                format!("{}{}", NAME_PREFIX, id)
            })
            .collect();
        let actor_index = i % actors.len();
        let actor_ref = actors[actor_index].actor_ref().dyn_ref();
        let res = (PathResolvable::Segments(path), actor_ref);
        paths.push(res);
    }
    DataSet {
        system,
        actors,
        paths,
    }
}

mod tests {
    use super::*;
    use criterion::{black_box, BatchSize, Bencher};

    pub fn bench_insert<L, F>(b: &mut Bencher, get_store: F, store_size: usize)
    where
        L: ActorLookup,
        F: Fn() -> L,
    {
        let mut data_set = load_data(store_size);
        b.iter_batched(
            || (get_store(), data_set.paths.clone()),
            |(mut store, paths)| {
                for (path, aref) in paths {
                    let res = store.insert(path, aref);
                    let _ = black_box(res);
                }
            },
            BatchSize::PerIteration,
        );
        data_set.paths.clear();
        data_set.actors.clear();
        data_set.system.shutdown().expect("shutdown");
    }

    pub fn bench_lookup<L>(b: &mut Bencher, mut store: L, store_size: usize)
    where
        L: ActorLookup,
    {
        let mut data_set = load_data(store_size);
        for (path, aref) in data_set.paths.iter() {
            let _ = store.insert(path.clone(), aref.clone());
        }
        let paths: Vec<&[String]> = data_set
            .paths
            .iter()
            .map(|(path, _)| {
                if let PathResolvable::Segments(ref segments) = path {
                    segments.as_slice()
                } else {
                    unreachable!("We only put in segments!")
                }
            })
            .collect();
        b.iter(|| {
            for path in paths.iter() {
                let res = store.get_by_named_path(path);
                let _ = black_box(res);
            }
        });
        drop(paths);
        data_set.paths.clear();
        data_set.actors.clear();
        data_set.system.shutdown().expect("shutdown");
    }

    pub fn bench_group_lookup<L>(b: &mut Bencher, mut store: L, store_size: usize)
    where
        L: ActorLookup,
    {
        let system = KompactConfig::default().build().expect("system");

        // set up system for routing
        let num_actors = 1.max(store_size / 3);
        let actors: Vec<Arc<Component<NopActor>>> = (0..num_actors)
            .map(|_| system.create(NopActor::default))
            .collect();
        let mut rng = rand::thread_rng();
        for i in 0..store_size {
            let path_len = rng.gen_range(1, MAX_DEPTH);
            let mut path: Vec<String> = (1..=path_len)
                .map(|depth| {
                    let level_width = 1.max(store_size / (depth * 2));
                    let id = i % level_width;
                    format!("{}{}", NAME_PREFIX, id)
                })
                .collect();
            // put everything under the router's path
            path.insert(0, ROUTER_PREFIX.to_string());
            let actor_index = i % actors.len();
            let actor_ref = actors[actor_index].actor_ref().dyn_ref();
            let _ = store.insert(PathResolvable::Segments(path), actor_ref);
        }

        let router_path = vec![ROUTER_PREFIX.to_string(), "*".to_string()];

        // benchmark
        b.iter_with_large_drop(|| store.get_by_named_path(black_box(&router_path)));
        drop(actors);
        system.shutdown().expect("shutdown");
    }

    pub fn bench_cleanup<L, F>(b: &mut Bencher, get_store: F, store_size: usize)
    where
        L: ActorLookup,
        F: Fn() -> L,
    {
        let mut data_set = load_data(store_size);
        let keep_len = 1.max(data_set.actors.len() / 2);
        data_set.actors.truncate(keep_len); // drop half of the actors, so they get cleaned up
        b.iter_batched(
            || {
                let mut store = get_store();
                for (path, aref) in data_set.paths.iter() {
                    let _ = store.insert(path.clone(), aref.clone());
                }
                store
            },
            |mut store| store.cleanup(),
            BatchSize::PerIteration,
        );
        data_set.paths.clear();
        data_set.actors.clear();
        data_set.system.shutdown().expect("shutdown");
    }
}

criterion_group!(
    actor_store_benches,
    insert_benches,
    lookup_benches,
    group_lookup_benches,
    cleanup_benches
);
criterion_main!(actor_store_benches);

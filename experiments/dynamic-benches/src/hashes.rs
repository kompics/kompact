use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use uuid::Uuid;

const DATA_SIZES: [usize; 4] = [1, 7, 100, 10000];

pub fn load_usize_data(data_size: usize) -> Vec<usize> {
    let mut v: Vec<usize> = Vec::with_capacity(data_size);
    for i in 0..data_size {
        v.push(i);
    }
    v
}

pub fn load_uuid_data(data_size: usize) -> Vec<Uuid> {
    let mut v: Vec<Uuid> = Vec::with_capacity(data_size);
    for _i in 0..data_size {
        v.push(Uuid::new_v4());
    }
    v
}

pub fn load_addr_data(data_size: usize) -> Vec<SocketAddr> {
    let mut v: Vec<SocketAddr> = Vec::with_capacity(data_size);
    for i in 0..data_size {
        let bytes: [u8; 4] = (i as u32).to_ne_bytes();
        let port_bytes = [bytes[0], bytes[1]];
        let port: u16 = unsafe { std::mem::transmute(port_bytes) };
        v.push(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(10, 0, bytes[3], bytes[2])),
            port,
        ));
    }
    v
}

pub fn socket_to_bytes(socket: &SocketAddr) -> [u8; 6] {
    let mut bytes = [0u8; 6];

    match socket {
        SocketAddr::V4(socket4) => {
            let octets = socket4.ip().octets();
            bytes[0] = octets[0];
            bytes[1] = octets[1];
            bytes[2] = octets[2];
            bytes[3] = octets[3];
        }
        _ => panic!("Only use V4 sockets!"),
    }

    let port: [u8; 2] = socket.port().to_ne_bytes();
    bytes[4] = port[0];
    bytes[5] = port[1];

    bytes
}

pub const VAL: &str = "Test me!";

pub fn insert_benches(c: &mut Criterion) {
    insert_benches_uuid(c);
    insert_benches_socket(c);
    insert_benches_usize(c);
}

pub fn insert_benches_uuid(c: &mut Criterion) {
    let mut g = c.benchmark_group("Hash Inserts UUID");
    for data_size in DATA_SIZES.iter() {
        g.bench_with_input(BenchmarkId::new("SIP", data_size), data_size, |b, &size| {
            tests::bench_uuid_insert_sip(b, size)
        });
        g.bench_with_input(BenchmarkId::new("FNV", data_size), data_size, |b, &size| {
            tests::bench_uuid_insert_fnv(b, size)
        });
        g.bench_with_input(BenchmarkId::new("FX", data_size), data_size, |b, &size| {
            tests::bench_uuid_insert_fx(b, size)
        });
        g.bench_with_input(
            BenchmarkId::new("FX-IM-INSERT", data_size),
            data_size,
            |b, &size| tests::bench_uuid_insert_fx_im_insert(b, size),
        );
        g.bench_with_input(
            BenchmarkId::new("FX-IM-UPDATE", data_size),
            data_size,
            |b, &size| tests::bench_uuid_insert_fx_im_update(b, size),
        );
        g.bench_with_input(BenchmarkId::new("XX", data_size), data_size, |b, &size| {
            tests::bench_uuid_insert_xx(b, size)
        });
        g.bench_with_input(
            BenchmarkId::new("BTree", data_size),
            data_size,
            |b, &size| tests::bench_uuid_insert_btree(b, size),
        );
        g.bench_with_input(
            BenchmarkId::new("Radix", data_size),
            data_size,
            |b, &size| tests::bench_uuid_insert_radix(b, size),
        );
        #[cfg(nightly)]
        g.bench_with_input(
            BenchmarkId::new("Byte Radix", data_size),
            data_size,
            |b, &size| tests::bench_uuid_insert_byteradix(b, size),
        );
    }
    g.finish();
}

pub fn insert_benches_socket(c: &mut Criterion) {
    let mut g = c.benchmark_group("Hash Inserts Socket");
    for data_size in DATA_SIZES.iter() {
        g.bench_with_input(BenchmarkId::new("SIP", data_size), data_size, |b, &size| {
            tests::bench_socket_insert_sip(b, size)
        });
        g.bench_with_input(BenchmarkId::new("FNV", data_size), data_size, |b, &size| {
            tests::bench_socket_insert_fnv(b, size)
        });
        g.bench_with_input(BenchmarkId::new("FX", data_size), data_size, |b, &size| {
            tests::bench_socket_insert_fx(b, size)
        });
        g.bench_with_input(BenchmarkId::new("XX", data_size), data_size, |b, &size| {
            tests::bench_socket_insert_xx(b, size)
        });
        g.bench_with_input(
            BenchmarkId::new("Radix", data_size),
            data_size,
            |b, &size| tests::bench_socket_insert_radix(b, size),
        );
        #[cfg(nightly)]
        g.bench_with_input(
            BenchmarkId::new("Byte Radix", data_size),
            data_size,
            |b, &size| tests::bench_socket_insert_byteradix(b, size),
        );
    }
    g.finish();
}

pub fn insert_benches_usize(c: &mut Criterion) {
    let mut g = c.benchmark_group("Hash Inserts usize");
    for data_size in DATA_SIZES.iter() {
        g.bench_with_input(BenchmarkId::new("SIP", data_size), data_size, |b, &size| {
            tests::bench_usize_insert_sip(b, size)
        });
        g.bench_with_input(BenchmarkId::new("FNV", data_size), data_size, |b, &size| {
            tests::bench_usize_insert_fnv(b, size)
        });
        g.bench_with_input(BenchmarkId::new("FX", data_size), data_size, |b, &size| {
            tests::bench_usize_insert_fx(b, size)
        });
        g.bench_with_input(BenchmarkId::new("XX", data_size), data_size, |b, &size| {
            tests::bench_usize_insert_xx(b, size)
        });
        g.bench_with_input(
            BenchmarkId::new("BTree", data_size),
            data_size,
            |b, &size| tests::bench_usize_insert_btree(b, size),
        );
        g.bench_with_input(
            BenchmarkId::new("Radix", data_size),
            data_size,
            |b, &size| tests::bench_usize_insert_radix(b, size),
        );
        #[cfg(nightly)]
        g.bench_with_input(
            BenchmarkId::new("Byte Radix", data_size),
            data_size,
            |b, &size| tests::bench_usize_insert_byteradix(b, size),
        );
    }
    g.finish();
}

pub fn lookup_benches(c: &mut Criterion) {
    lookup_benches_uuid(c);
    lookup_benches_socket(c);
    lookup_benches_usize(c);
}

pub fn lookup_benches_uuid(c: &mut Criterion) {
    let mut g = c.benchmark_group("Hash Lookups UUID");
    for data_size in DATA_SIZES.iter() {
        g.bench_with_input(BenchmarkId::new("SIP", data_size), data_size, |b, &size| {
            tests::bench_uuid_lookup_sip(b, size)
        });
        g.bench_with_input(BenchmarkId::new("FNV", data_size), data_size, |b, &size| {
            tests::bench_uuid_lookup_fnv(b, size)
        });
        g.bench_with_input(BenchmarkId::new("FX", data_size), data_size, |b, &size| {
            tests::bench_uuid_lookup_fx(b, size)
        });
        g.bench_with_input(
            BenchmarkId::new("FX-IM", data_size),
            data_size,
            |b, &size| tests::bench_uuid_lookup_fx_im(b, size),
        );
        g.bench_with_input(BenchmarkId::new("XX", data_size), data_size, |b, &size| {
            tests::bench_uuid_lookup_xx(b, size)
        });
        g.bench_with_input(
            BenchmarkId::new("BTree", data_size),
            data_size,
            |b, &size| tests::bench_uuid_lookup_btree(b, size),
        );
        g.bench_with_input(
            BenchmarkId::new("Radix", data_size),
            data_size,
            |b, &size| tests::bench_uuid_lookup_radix(b, size),
        );
        #[cfg(nightly)]
        g.bench_with_input(
            BenchmarkId::new("Byte Radix", data_size),
            data_size,
            |b, &size| tests::bench_uuid_lookup_byteradix(b, size),
        );
    }
    g.finish();
}

pub fn lookup_benches_socket(c: &mut Criterion) {
    let mut g = c.benchmark_group("Hash Lookups Socket");
    for data_size in DATA_SIZES.iter() {
        g.bench_with_input(BenchmarkId::new("SIP", data_size), data_size, |b, &size| {
            tests::bench_socket_lookup_sip(b, size)
        });
        g.bench_with_input(BenchmarkId::new("FNV", data_size), data_size, |b, &size| {
            tests::bench_socket_lookup_fnv(b, size)
        });
        g.bench_with_input(BenchmarkId::new("FX", data_size), data_size, |b, &size| {
            tests::bench_socket_lookup_fx(b, size)
        });
        g.bench_with_input(BenchmarkId::new("XX", data_size), data_size, |b, &size| {
            tests::bench_socket_lookup_xx(b, size)
        });
        g.bench_with_input(
            BenchmarkId::new("Radix", data_size),
            data_size,
            |b, &size| tests::bench_socket_lookup_radix(b, size),
        );
        #[cfg(nightly)]
        g.bench_with_input(
            BenchmarkId::new("ByteRadix", data_size),
            data_size,
            |b, &size| tests::bench_socket_lookup_byteradix(b, size),
        );
    }
    g.finish();
}

pub fn lookup_benches_usize(c: &mut Criterion) {
    let mut g = c.benchmark_group("Hash Lookups usize");
    for data_size in DATA_SIZES.iter() {
        g.bench_with_input(BenchmarkId::new("SIP", data_size), data_size, |b, &size| {
            tests::bench_usize_lookup_sip(b, size)
        });
        g.bench_with_input(BenchmarkId::new("FNV", data_size), data_size, |b, &size| {
            tests::bench_usize_lookup_fnv(b, size)
        });
        g.bench_with_input(BenchmarkId::new("FX", data_size), data_size, |b, &size| {
            tests::bench_usize_lookup_fx(b, size)
        });
        g.bench_with_input(BenchmarkId::new("XX", data_size), data_size, |b, &size| {
            tests::bench_usize_lookup_xx(b, size)
        });
        g.bench_with_input(
            BenchmarkId::new("BTree", data_size),
            data_size,
            |b, &size| tests::bench_usize_lookup_btree(b, size),
        );
        g.bench_with_input(
            BenchmarkId::new("Radix", data_size),
            data_size,
            |b, &size| tests::bench_usize_lookup_radix(b, size),
        );
        #[cfg(nightly)]
        g.bench_with_input(
            BenchmarkId::new("Byte Radix", data_size),
            data_size,
            |b, &size| tests::bench_usize_lookup_byteradix(b, size),
        );
    }
    g.finish();
}

mod tests {
    use super::*;
    use criterion::Bencher;
    #[cfg(nightly)]
    use datastructures::ByteSliceMap;
    use fnv::FnvHashMap;
    use im::HashMap as ImmutableHashMap;
    use panoradix::RadixMap;
    use rustc_hash::{FxHashMap, FxHasher};
    use std::{
        collections::{BTreeMap, HashMap},
        hash::BuildHasherDefault,
    };
    use twox_hash::XxHash64;

    pub fn bench_uuid_insert_sip(b: &mut Bencher, data_size: usize) {
        let data = load_uuid_data(data_size);
        let mut map: HashMap<Uuid, &'static str> =
            HashMap::with_capacity_and_hasher(data_size, Default::default());
        b.iter(|| {
            data.iter().for_each(|id| {
                let _ = map.insert(*id, VAL);
            });
        });
        assert_eq!(map.len(), data_size);
        map.drain().for_each(|v| {
            assert_eq!(v.1, VAL);
        });
    }

    pub fn bench_uuid_insert_fnv(b: &mut Bencher, data_size: usize) {
        let data = load_uuid_data(data_size);
        let mut map: FnvHashMap<Uuid, &'static str> =
            FnvHashMap::with_capacity_and_hasher(data_size, Default::default());
        b.iter(|| {
            data.iter().for_each(|id| {
                let _ = map.insert(*id, VAL);
            });
        });
        assert_eq!(map.len(), data_size);
        map.drain().for_each(|v| {
            assert_eq!(v.1, VAL);
        });
    }

    pub fn bench_uuid_insert_fx(b: &mut Bencher, data_size: usize) {
        let data = load_uuid_data(data_size);
        let mut map: FxHashMap<Uuid, &'static str> =
            FxHashMap::with_capacity_and_hasher(data_size, Default::default());
        b.iter(|| {
            data.iter().for_each(|id| {
                let _ = map.insert(*id, VAL);
            });
        });
        assert_eq!(map.len(), data_size);
        map.drain().for_each(|v| {
            assert_eq!(v.1, VAL);
        });
    }

    pub fn bench_uuid_insert_fx_im_insert(b: &mut Bencher, data_size: usize) {
        let data = load_uuid_data(data_size);
        let mut map: ImmutableHashMap<Uuid, &'static str, BuildHasherDefault<FxHasher>> =
            ImmutableHashMap::default();
        b.iter(|| {
            data.iter().for_each(|id| {
                let _ = map.insert(*id, VAL);
            });
        });
        assert_eq!(map.len(), data_size);
        map.iter().for_each(|v| {
            assert_eq!(*v.1, VAL);
        });
    }

    pub fn bench_uuid_insert_fx_im_update(b: &mut Bencher, data_size: usize) {
        let data = load_uuid_data(data_size);
        let mut map: ImmutableHashMap<Uuid, &'static str, BuildHasherDefault<FxHasher>> =
            ImmutableHashMap::default();
        b.iter(|| {
            data.iter().for_each(|id| {
                map = map.update(*id, VAL);
            });
        });
        assert_eq!(map.len(), data_size);
        map.iter().for_each(|v| {
            assert_eq!(*v.1, VAL);
        });
    }

    pub fn bench_uuid_insert_xx(b: &mut Bencher, data_size: usize) {
        let data = load_uuid_data(data_size);
        let mut map: HashMap<Uuid, &'static str, BuildHasherDefault<XxHash64>> = Default::default();
        b.iter(|| {
            data.iter().for_each(|id| {
                let _ = map.insert(*id, VAL);
            });
        });
        assert_eq!(map.len(), data_size);
        map.drain().for_each(|v| {
            assert_eq!(v.1, VAL);
        });
    }

    pub fn bench_uuid_insert_btree(b: &mut Bencher, data_size: usize) {
        let data = load_uuid_data(data_size);
        let mut map: BTreeMap<Uuid, &'static str> = BTreeMap::new();
        b.iter(|| {
            data.iter().for_each(|id| {
                let _ = map.insert(*id, VAL);
            });
        });
        assert_eq!(map.len(), data_size);
        map.iter().for_each(|v| {
            assert_eq!(*v.1, VAL);
        });
    }

    pub fn bench_uuid_insert_radix(b: &mut Bencher, data_size: usize) {
        let data = load_uuid_data(data_size);
        let mut map: RadixMap<[u8], &'static str> = RadixMap::new();
        b.iter(|| {
            data.iter().for_each(|id| {
                let _ = map.insert(id.as_bytes(), VAL);
            });
        });
        //assert_eq!(map.len(), data_size);
        map.iter().for_each(|v| {
            assert_eq!(*v.1, VAL);
        });
    }

    #[cfg(nightly)]
    pub fn bench_uuid_insert_byteradix(b: &mut Bencher, data_size: usize) {
        let data = load_uuid_data(data_size);
        let mut map: ByteSliceMap<&'static str> = ByteSliceMap::new();
        b.iter(|| {
            data.iter().for_each(|id| {
                let _ = map.insert(id.as_bytes(), VAL);
            });
        });
        //assert_eq!(map.len(), data_size);
        // map.iter().for_each(|v| {
        //     assert_eq!(*v.1, VAL);
        // });
    }

    pub fn bench_socket_insert_sip(b: &mut Bencher, data_size: usize) {
        let data = load_addr_data(data_size);
        let mut map: HashMap<SocketAddr, &'static str> =
            HashMap::with_capacity_and_hasher(data_size, Default::default());
        b.iter(|| {
            data.iter().for_each(|id| {
                let _ = map.insert(*id, VAL);
            });
        });
        assert_eq!(map.len(), data_size);
        map.drain().for_each(|v| {
            assert_eq!(v.1, VAL);
        });
    }

    pub fn bench_socket_insert_fnv(b: &mut Bencher, data_size: usize) {
        let data = load_addr_data(data_size);
        let mut map: FnvHashMap<SocketAddr, &'static str> =
            FnvHashMap::with_capacity_and_hasher(data_size, Default::default());
        b.iter(|| {
            data.iter().for_each(|id| {
                let _ = map.insert(*id, VAL);
            });
        });
        assert_eq!(map.len(), data_size);
        map.drain().for_each(|v| {
            assert_eq!(v.1, VAL);
        });
    }

    pub fn bench_socket_insert_fx(b: &mut Bencher, data_size: usize) {
        let data = load_addr_data(data_size);
        let mut map: FxHashMap<SocketAddr, &'static str> =
            FxHashMap::with_capacity_and_hasher(data_size, Default::default());
        b.iter(|| {
            data.iter().for_each(|id| {
                let _ = map.insert(*id, VAL);
            });
        });
        assert_eq!(map.len(), data_size);
        map.drain().for_each(|v| {
            assert_eq!(v.1, VAL);
        });
    }

    pub fn bench_socket_insert_xx(b: &mut Bencher, data_size: usize) {
        let data = load_addr_data(data_size);
        let mut map: HashMap<SocketAddr, &'static str, BuildHasherDefault<XxHash64>> =
            Default::default();
        b.iter(|| {
            data.iter().for_each(|id| {
                let _ = map.insert(*id, VAL);
            });
        });
        assert_eq!(map.len(), data_size);
        map.drain().for_each(|v| {
            assert_eq!(v.1, VAL);
        });
    }

    pub fn bench_socket_insert_radix(b: &mut Bencher, data_size: usize) {
        let data = load_addr_data(data_size);
        let mut map: RadixMap<[u8], &'static str> = RadixMap::new();
        b.iter(|| {
            data.iter().for_each(|id| {
                let key = socket_to_bytes(id);
                let _ = map.insert(&key, VAL);
            });
        });
        //assert_eq!(map.len(), data_size);
        map.iter().for_each(|v| {
            assert_eq!(*v.1, VAL);
        });
    }

    #[cfg(nightly)]
    pub fn bench_socket_insert_byteradix(b: &mut Bencher, data_size: usize) {
        let data = load_addr_data(data_size);
        let mut map: ByteSliceMap<&'static str> = ByteSliceMap::new();
        b.iter(|| {
            data.iter().for_each(|id| {
                let key = socket_to_bytes(id);
                let _ = map.insert(&key, VAL);
            });
        });
        //assert_eq!(map.len(), data_size);
        // map.iter().for_each(|v| {
        //     assert_eq!(*v.1, VAL);
        // });
    }

    pub fn bench_usize_insert_sip(b: &mut Bencher, data_size: usize) {
        let data = load_usize_data(data_size);
        let mut map: HashMap<usize, &'static str> =
            HashMap::with_capacity_and_hasher(data_size, Default::default());
        b.iter(|| {
            data.iter().for_each(|id| {
                let _ = map.insert(*id, VAL);
            });
        });
        assert_eq!(map.len(), data_size);
        map.drain().for_each(|v| {
            assert_eq!(v.1, VAL);
        });
    }

    pub fn bench_usize_insert_fnv(b: &mut Bencher, data_size: usize) {
        let data = load_usize_data(data_size);
        let mut map: FnvHashMap<usize, &'static str> =
            FnvHashMap::with_capacity_and_hasher(data_size, Default::default());
        b.iter(|| {
            data.iter().for_each(|id| {
                let _ = map.insert(*id, VAL);
            });
        });
        assert_eq!(map.len(), data_size);
        map.drain().for_each(|v| {
            assert_eq!(v.1, VAL);
        });
    }

    pub fn bench_usize_insert_fx(b: &mut Bencher, data_size: usize) {
        let data = load_usize_data(data_size);
        let mut map: FxHashMap<usize, &'static str> =
            FxHashMap::with_capacity_and_hasher(data_size, Default::default());
        b.iter(|| {
            data.iter().for_each(|id| {
                let _ = map.insert(*id, VAL);
            });
        });
        assert_eq!(map.len(), data_size);
        map.drain().for_each(|v| {
            assert_eq!(v.1, VAL);
        });
    }

    pub fn bench_usize_insert_xx(b: &mut Bencher, data_size: usize) {
        let data = load_usize_data(data_size);
        let mut map: HashMap<usize, &'static str, BuildHasherDefault<XxHash64>> =
            Default::default();
        b.iter(|| {
            data.iter().for_each(|id| {
                let _ = map.insert(*id, VAL);
            });
        });
        assert_eq!(map.len(), data_size);
        map.drain().for_each(|v| {
            assert_eq!(v.1, VAL);
        });
    }

    pub fn bench_usize_insert_btree(b: &mut Bencher, data_size: usize) {
        let data = load_usize_data(data_size);
        let mut map: BTreeMap<usize, &'static str> = BTreeMap::new();
        b.iter(|| {
            data.iter().for_each(|id| {
                let _ = map.insert(*id, VAL);
            });
        });
        assert_eq!(map.len(), data_size);
        map.iter().for_each(|v| {
            assert_eq!(*v.1, VAL);
        });
    }

    pub fn bench_usize_insert_radix(b: &mut Bencher, data_size: usize) {
        let data = load_usize_data(data_size);
        let mut map: RadixMap<[u8], &'static str> = RadixMap::new();
        b.iter(|| {
            data.iter().for_each(|id| {
                let key: [u8; 8] = unsafe { std::mem::transmute(id) };
                let _ = map.insert(&key, VAL);
            });
        });
        //assert_eq!(map.len(), data_size);
        map.iter().for_each(|v| {
            assert_eq!(*v.1, VAL);
        });
    }

    #[cfg(nightly)]
    pub fn bench_usize_insert_byteradix(b: &mut Bencher, data_size: usize) {
        let data = load_usize_data(data_size);
        let mut map: ByteSliceMap<&'static str> = ByteSliceMap::new();
        b.iter(|| {
            data.iter().for_each(|id| {
                let key: [u8; 8] = unsafe { std::mem::transmute(id) };
                let _ = map.insert(&key, VAL);
            });
        });
        //assert_eq!(map.len(), data_size);
        // map.iter().for_each(|v| {
        //     assert_eq!(*v.1, VAL);
        // });
    }

    // LOOKUPS

    pub fn bench_uuid_lookup_sip(b: &mut Bencher, data_size: usize) {
        let data = load_uuid_data(data_size);
        let mut map: HashMap<Uuid, &'static str> =
            HashMap::with_capacity_and_hasher(data_size, Default::default());
        data.iter().for_each(|id| {
            let _ = map.insert(*id, VAL);
        });
        assert_eq!(map.len(), data_size);
        b.iter(|| {
            data.iter().for_each(|id| {
                let r = map.get(id);
                assert!(r.is_some());
            });
        });
    }

    pub fn bench_uuid_lookup_fnv(b: &mut Bencher, data_size: usize) {
        let data = load_uuid_data(data_size);
        let mut map: FnvHashMap<Uuid, &'static str> =
            FnvHashMap::with_capacity_and_hasher(data_size, Default::default());
        data.iter().for_each(|id| {
            let _ = map.insert(*id, VAL);
        });
        assert_eq!(map.len(), data_size);
        b.iter(|| {
            data.iter().for_each(|id| {
                let r = map.get(id);
                assert!(r.is_some());
            });
        });
    }

    pub fn bench_uuid_lookup_fx(b: &mut Bencher, data_size: usize) {
        let data = load_uuid_data(data_size);
        let mut map: FxHashMap<Uuid, &'static str> =
            FxHashMap::with_capacity_and_hasher(data_size, Default::default());
        data.iter().for_each(|id| {
            let _ = map.insert(*id, VAL);
        });
        assert_eq!(map.len(), data_size);
        b.iter(|| {
            data.iter().for_each(|id| {
                let r = map.get(id);
                assert!(r.is_some());
            });
        });
    }

    pub fn bench_uuid_lookup_fx_im(b: &mut Bencher, data_size: usize) {
        let data = load_uuid_data(data_size);
        let mut map: ImmutableHashMap<Uuid, &'static str, BuildHasherDefault<FxHasher>> =
            ImmutableHashMap::default();
        data.iter().for_each(|id| {
            let _ = map.insert(*id, VAL);
        });
        assert_eq!(map.len(), data_size);
        b.iter(|| {
            data.iter().for_each(|id| {
                let r = map.get(id);
                assert!(r.is_some());
            });
        });
    }

    pub fn bench_uuid_lookup_xx(b: &mut Bencher, data_size: usize) {
        let data = load_uuid_data(data_size);
        let mut map: HashMap<Uuid, &'static str, BuildHasherDefault<XxHash64>> = Default::default();
        data.iter().for_each(|id| {
            let _ = map.insert(*id, VAL);
        });
        assert_eq!(map.len(), data_size);
        b.iter(|| {
            data.iter().for_each(|id| {
                let r = map.get(id);
                assert!(r.is_some());
            });
        });
    }

    pub fn bench_uuid_lookup_btree(b: &mut Bencher, data_size: usize) {
        let data = load_uuid_data(data_size);
        let mut map: BTreeMap<Uuid, &'static str> = BTreeMap::new();
        data.iter().for_each(|id| {
            let _ = map.insert(*id, VAL);
        });
        assert_eq!(map.len(), data_size);
        b.iter(|| {
            data.iter().for_each(|id| {
                let r = map.get(id);
                assert!(r.is_some());
            });
        });
    }

    pub fn bench_uuid_lookup_radix(b: &mut Bencher, data_size: usize) {
        let data = load_uuid_data(data_size);
        let mut map: RadixMap<[u8], &'static str> = RadixMap::new();
        data.iter().for_each(|id| {
            let _ = map.insert(id.as_bytes(), VAL);
        });
        //assert_eq!(map.len(), data_size);
        b.iter(|| {
            data.iter().for_each(|id| {
                let r = map.get(id.as_bytes());
                assert!(r.is_some());
            });
        });
    }

    #[cfg(nightly)]
    pub fn bench_uuid_lookup_byteradix(b: &mut Bencher, data_size: usize) {
        let data = load_uuid_data(data_size);
        let mut map: ByteSliceMap<&'static str> = ByteSliceMap::new();
        data.iter().for_each(|id| {
            let _ = map.insert(id.as_bytes(), VAL);
        });
        //assert_eq!(map.len(), data_size);
        b.iter(|| {
            data.iter().for_each(|id| {
                let r = map.get(id.as_bytes());
                assert!(r.is_some());
            });
        });
    }

    pub fn bench_socket_lookup_sip(b: &mut Bencher, data_size: usize) {
        let data = load_addr_data(data_size);
        let mut map: HashMap<SocketAddr, &'static str> =
            HashMap::with_capacity_and_hasher(data_size, Default::default());
        data.iter().for_each(|id| {
            let _ = map.insert(*id, VAL);
        });
        assert_eq!(map.len(), data_size);
        b.iter(|| {
            data.iter().for_each(|id| {
                let r = map.get(id);
                assert!(r.is_some());
            });
        });
    }

    pub fn bench_socket_lookup_fnv(b: &mut Bencher, data_size: usize) {
        let data = load_addr_data(data_size);
        let mut map: FnvHashMap<SocketAddr, &'static str> =
            FnvHashMap::with_capacity_and_hasher(data_size, Default::default());
        data.iter().for_each(|id| {
            let _ = map.insert(*id, VAL);
        });
        assert_eq!(map.len(), data_size);
        b.iter(|| {
            data.iter().for_each(|id| {
                let r = map.get(id);
                assert!(r.is_some());
            });
        });
    }

    pub fn bench_socket_lookup_fx(b: &mut Bencher, data_size: usize) {
        let data = load_addr_data(data_size);
        let mut map: FxHashMap<SocketAddr, &'static str> =
            FxHashMap::with_capacity_and_hasher(data_size, Default::default());
        data.iter().for_each(|id| {
            let _ = map.insert(*id, VAL);
        });
        assert_eq!(map.len(), data_size);
        b.iter(|| {
            data.iter().for_each(|id| {
                let r = map.get(id);
                assert!(r.is_some());
            });
        });
    }

    pub fn bench_socket_lookup_xx(b: &mut Bencher, data_size: usize) {
        let data = load_addr_data(data_size);
        let mut map: HashMap<SocketAddr, &'static str, BuildHasherDefault<XxHash64>> =
            Default::default();
        data.iter().for_each(|id| {
            let _ = map.insert(*id, VAL);
        });
        assert_eq!(map.len(), data_size);
        b.iter(|| {
            data.iter().for_each(|id| {
                let r = map.get(id);
                assert!(r.is_some());
            });
        });
    }

    pub fn bench_socket_lookup_radix(b: &mut Bencher, data_size: usize) {
        let data = load_addr_data(data_size);
        let mut map: RadixMap<[u8], &'static str> = RadixMap::new();
        data.iter().for_each(|id| {
            let key = socket_to_bytes(id);
            let _ = map.insert(&key, VAL);
        });
        //assert_eq!(map.len(), data_size);
        b.iter(|| {
            data.iter().for_each(|id| {
                let key = socket_to_bytes(id);
                let r = map.get(&key);
                assert!(r.is_some());
            });
        });
    }

    #[cfg(nightly)]
    pub fn bench_socket_lookup_byteradix(b: &mut Bencher, data_size: usize) {
        let data = load_addr_data(data_size);
        let mut map: ByteSliceMap<&'static str> = ByteSliceMap::new();
        data.iter().for_each(|id| {
            let key = socket_to_bytes(id);
            let _ = map.insert(&key, VAL);
        });
        //assert_eq!(map.len(), data_size);
        b.iter(|| {
            data.iter().for_each(|id| {
                let key = socket_to_bytes(id);
                let r = map.get(&key);
                assert!(r.is_some());
            });
        });
    }

    pub fn bench_usize_lookup_sip(b: &mut Bencher, data_size: usize) {
        let data = load_usize_data(data_size);
        let mut map: HashMap<usize, &'static str> =
            HashMap::with_capacity_and_hasher(data_size, Default::default());
        data.iter().for_each(|id| {
            let _ = map.insert(*id, VAL);
        });
        assert_eq!(map.len(), data_size);
        b.iter(|| {
            data.iter().for_each(|id| {
                let r = map.get(id);
                assert!(r.is_some());
            });
        });
    }

    pub fn bench_usize_lookup_fnv(b: &mut Bencher, data_size: usize) {
        let data = load_usize_data(data_size);
        let mut map: FnvHashMap<usize, &'static str> =
            FnvHashMap::with_capacity_and_hasher(data_size, Default::default());
        data.iter().for_each(|id| {
            let _ = map.insert(*id, VAL);
        });
        assert_eq!(map.len(), data_size);
        b.iter(|| {
            data.iter().for_each(|id| {
                let r = map.get(id);
                assert!(r.is_some());
            });
        });
    }

    pub fn bench_usize_lookup_fx(b: &mut Bencher, data_size: usize) {
        let data = load_usize_data(data_size);
        let mut map: FxHashMap<usize, &'static str> =
            FxHashMap::with_capacity_and_hasher(data_size, Default::default());
        data.iter().for_each(|id| {
            let _ = map.insert(*id, VAL);
        });
        assert_eq!(map.len(), data_size);
        b.iter(|| {
            data.iter().for_each(|id| {
                let r = map.get(id);
                assert!(r.is_some());
            });
        });
    }

    pub fn bench_usize_lookup_xx(b: &mut Bencher, data_size: usize) {
        let data = load_usize_data(data_size);
        let mut map: HashMap<usize, &'static str, BuildHasherDefault<XxHash64>> =
            Default::default();
        data.iter().for_each(|id| {
            let _ = map.insert(*id, VAL);
        });
        assert_eq!(map.len(), data_size);
        b.iter(|| {
            data.iter().for_each(|id| {
                let r = map.get(id);
                assert!(r.is_some());
            });
        });
    }

    pub fn bench_usize_lookup_btree(b: &mut Bencher, data_size: usize) {
        let data = load_usize_data(data_size);
        let mut map: BTreeMap<usize, &'static str> = BTreeMap::new();
        data.iter().for_each(|id| {
            let _ = map.insert(*id, VAL);
        });
        assert_eq!(map.len(), data_size);
        b.iter(|| {
            data.iter().for_each(|id| {
                let r = map.get(id);
                assert!(r.is_some());
            });
        });
    }

    pub fn bench_usize_lookup_radix(b: &mut Bencher, data_size: usize) {
        let data = load_usize_data(data_size);
        let mut map: RadixMap<[u8], &'static str> = RadixMap::new();
        data.iter().for_each(|id| {
            let key: [u8; 8] = unsafe { std::mem::transmute(id) };
            let _ = map.insert(&key, VAL);
        });
        //assert_eq!(map.len(), data_size);
        b.iter(|| {
            data.iter().for_each(|id| {
                let key: [u8; 8] = unsafe { std::mem::transmute(id) };
                let r = map.get(&key);
                assert!(r.is_some());
            });
        });
    }

    #[cfg(nightly)]
    pub fn bench_usize_lookup_byteradix(b: &mut Bencher, data_size: usize) {
        let data = load_usize_data(data_size);
        let mut map: ByteSliceMap<&'static str> = ByteSliceMap::new();
        data.iter().for_each(|id| {
            let key: [u8; 8] = unsafe { std::mem::transmute(id) };
            let _ = map.insert(&key, VAL);
        });
        //assert_eq!(map.len(), data_size);
        b.iter(|| {
            data.iter().for_each(|id| {
                let key: [u8; 8] = unsafe { std::mem::transmute(id) };
                let r = map.get(&key);
                assert!(r.is_some());
            });
        });
    }
}

criterion_group!(hash_benches, insert_benches, lookup_benches);
criterion_main!(hash_benches);

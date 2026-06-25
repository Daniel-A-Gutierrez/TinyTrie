use std::collections::{BTreeMap, HashMap};
use std::hash::Hash;
use std::hint::black_box;

use super::{Benchable, BenchCtx, build_sorted_vec, read_allocated, sorted_vec_get};

// These three std containers are generic over their key type, so each carries
// both a `bytes` and a `u64` variant — letting them run on their NATIVE key type
// in every mode instead of a `u64`-as-byte-string projection. The byte-string
// *tries* (NibbleTrie, BitTrie, …) cannot do this, so they sit out `u64` modes;
// only these generic containers (and CTree's u64 variant) run there.

// ── BTreeMap ───────────────────────────────────────────────────────────

pub(crate) struct BTreeMapBenchGen<K> {
    map: BTreeMap<K, usize>,
}

impl<K: Ord + Clone + 'static> BTreeMapBenchGen<K> {
    pub(crate) fn new() -> Self { Self { map: BTreeMap::new() } }
}

impl<K: Ord + Clone + 'static> Benchable<K> for BTreeMapBenchGen<K> {
    fn build(&mut self, keys: &[K], _ctx: &BenchCtx<K>) {
        self.map = BTreeMap::new();
        for (i, k) in keys.iter().enumerate() { self.map.insert(k.clone(), i); }
    }

    fn bench_insert(&self, keys: &[K]) -> Option<()> {
        let mut m = BTreeMap::new();
        for (i, k) in keys.iter().enumerate() { m.insert(k.clone(), i); }
        black_box(&m);
        Some(())
    }

    fn bench_lookup(&self, ctx: &BenchCtx<K>) -> Option<()> {
        for k in &ctx.lookup_keys { black_box(self.map.get(k)); }
        Some(())
    }

    fn bench_fwd_iter(&self) -> Option<()> {
        for (k, v) in self.map.iter() { black_box(k); black_box(v); }
        Some(())
    }

    fn bench_rev_iter(&self) -> Option<()> {
        for (k, v) in self.map.iter().rev() { black_box(k); black_box(v); }
        Some(())
    }

    fn bench_memory(&self, keys: &[K]) -> Option<f64> {
        let before = read_allocated();
        let mut m: BTreeMap<K, usize> = BTreeMap::new();
        for (i, k) in keys.iter().enumerate() { m.insert(k.clone(), i); }
        let bytes = read_allocated() - before;
        drop(m);
        Some(bytes as f64 / keys.len() as f64)
    }
}

pub(crate) type BTreeMapBench = BTreeMapBenchGen<Vec<u8>>;
pub(crate) type BTreeMapBenchU64 = BTreeMapBenchGen<u64>;

// ── HashMap ────────────────────────────────────────────────────────────

pub(crate) struct HashMapBenchGen<K> {
    map: HashMap<K, usize>,
}

impl<K: Hash + Eq + Clone + 'static> HashMapBenchGen<K> {
    pub(crate) fn new() -> Self { Self { map: HashMap::new() } }
}

impl<K: Hash + Eq + Clone + 'static> Benchable<K> for HashMapBenchGen<K> {
    fn build(&mut self, keys: &[K], _ctx: &BenchCtx<K>) {
        self.map = HashMap::new();
        for (i, k) in keys.iter().enumerate() { self.map.insert(k.clone(), i); }
    }

    fn bench_insert(&self, keys: &[K]) -> Option<()> {
        let mut m = HashMap::new();
        for (i, k) in keys.iter().enumerate() { m.insert(k.clone(), i); }
        black_box(&m);
        Some(())
    }

    fn bench_lookup(&self, ctx: &BenchCtx<K>) -> Option<()> {
        for k in &ctx.lookup_keys { black_box(self.map.get(k)); }
        Some(())
    }

    fn bench_memory(&self, keys: &[K]) -> Option<f64> {
        let before = read_allocated();
        let mut m: HashMap<K, usize> = HashMap::new();
        for (i, k) in keys.iter().enumerate() { m.insert(k.clone(), i); }
        let bytes = read_allocated() - before;
        drop(m);
        Some(bytes as f64 / keys.len() as f64)
    }
}

pub(crate) type HashMapBench = HashMapBenchGen<Vec<u8>>;
pub(crate) type HashMapBenchU64 = HashMapBenchGen<u64>;

// ── SortedVec ──────────────────────────────────────────────────────────

pub(crate) struct SortedVecBenchGen<K> {
    sorted: Vec<(K, usize)>,
}

impl<K: Ord + Clone + 'static> SortedVecBenchGen<K> {
    pub(crate) fn new() -> Self { Self { sorted: Vec::new() } }
}

impl<K: Ord + Clone + 'static> Benchable<K> for SortedVecBenchGen<K> {
    fn build(&mut self, keys: &[K], _ctx: &BenchCtx<K>) {
        self.sorted = build_sorted_vec(keys);
    }

    fn bench_insert(&self, keys: &[K]) -> Option<()> {
        let mut v: Vec<(K, usize)> = Vec::new();
        for (i, k) in keys.iter().enumerate() {
            match v.binary_search_by(|e| e.0.cmp(k)) {
                Ok(_) => {}
                Err(pos) => v.insert(pos, (k.clone(), i)),
            }
        }
        black_box(&v);
        Some(())
    }

    fn bench_lookup(&self, ctx: &BenchCtx<K>) -> Option<()> {
        for k in &ctx.lookup_keys { black_box(sorted_vec_get(&self.sorted, k)); }
        Some(())
    }

    fn bench_fwd_iter(&self) -> Option<()> {
        for (k, v) in self.sorted.iter() { black_box(k); black_box(v); }
        Some(())
    }

    fn bench_memory(&self, keys: &[K]) -> Option<f64> {
        let before = read_allocated();
        let s = build_sorted_vec(keys);
        let bytes = read_allocated() - before;
        drop(s);
        Some(bytes as f64 / keys.len() as f64)
    }
}

pub(crate) type SortedVecBench = SortedVecBenchGen<Vec<u8>>;
pub(crate) type SortedVecBenchU64 = SortedVecBenchGen<u64>;
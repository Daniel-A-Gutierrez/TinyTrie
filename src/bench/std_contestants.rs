use std::collections::{BTreeMap, HashMap, LinkedList};
use std::hint::black_box;

use super::{Benchable, BenchContext, build_sorted_vec, sorted_vec_get, read_allocated};

pub(crate) struct BTreeMapBench {
    map: BTreeMap<Vec<u8>, usize>,
}

impl BTreeMapBench {
    pub(crate) fn new() -> Self { Self { map: BTreeMap::new() } }
}

impl Benchable for BTreeMapBench {
    fn build(&mut self, keys: &[Vec<u8>], _ctx: &BenchContext) {
        self.map = BTreeMap::new();
        for (i, k) in keys.iter().enumerate() { self.map.insert(k.clone(), i); }
    }

    fn bench_insert(&self, keys: &[Vec<u8>]) -> Option<()> {
        let mut m = BTreeMap::new();
        for (i, k) in keys.iter().enumerate() { m.insert(k.clone(), i); }
        black_box(&m);
        Some(())
    }

    fn bench_lookup(&self, ctx: &BenchContext) -> Option<()> {
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

    fn bench_memory(&self, keys: &[Vec<u8>]) -> Option<f64> {
        let before = read_allocated();
        let mut m: BTreeMap<Vec<u8>, usize> = BTreeMap::new();
        for (i, k) in keys.iter().enumerate() { m.insert(k.clone(), i); }
        let bytes = read_allocated() - before;
        drop(m);
        Some(bytes as f64 / keys.len() as f64)
    }
}

// ── HashMap ────────────────────────────────────────────────────────────

pub(crate) struct HashMapBench {
    map: HashMap<Vec<u8>, usize>,
}

impl HashMapBench {
    pub(crate) fn new() -> Self { Self { map: HashMap::new() } }
}

impl Benchable for HashMapBench {
    fn build(&mut self, keys: &[Vec<u8>], _ctx: &BenchContext) {
        self.map = HashMap::new();
        for (i, k) in keys.iter().enumerate() { self.map.insert(k.clone(), i); }
    }

    fn bench_insert(&self, keys: &[Vec<u8>]) -> Option<()> {
        let mut m = HashMap::new();
        for (i, k) in keys.iter().enumerate() { m.insert(k.clone(), i); }
        black_box(&m);
        Some(())
    }

    fn bench_lookup(&self, ctx: &BenchContext) -> Option<()> {
        for k in &ctx.lookup_keys { black_box(self.map.get(k)); }
        Some(())
    }

    fn bench_memory(&self, keys: &[Vec<u8>]) -> Option<f64> {
        let before = read_allocated();
        let mut m: HashMap<Vec<u8>, usize> = HashMap::new();
        for (i, k) in keys.iter().enumerate() { m.insert(k.clone(), i); }
        let bytes = read_allocated() - before;
        drop(m);
        Some(bytes as f64 / keys.len() as f64)
    }
}

// ── SortedVec ──────────────────────────────────────────────────────────

pub(crate) struct SortedVecBench {
    sorted: Vec<(Vec<u8>, usize)>,
}

impl SortedVecBench {
    pub(crate) fn new() -> Self { Self { sorted: Vec::new() } }
}

impl Benchable for SortedVecBench {
    fn build(&mut self, keys: &[Vec<u8>], _ctx: &BenchContext) {
        self.sorted = build_sorted_vec(keys);
    }

    fn bench_insert(&self, keys: &[Vec<u8>]) -> Option<()> {
        let mut v: Vec<(Vec<u8>, usize)> = Vec::new();
        for (i, k) in keys.iter().enumerate() {
            match v.binary_search_by(|e| e.0.as_slice().cmp(k)) {
                Ok(_) => {}
                Err(pos) => v.insert(pos, (k.clone(), i)),
            }
        }
        black_box(&v);
        Some(())
    }

    fn bench_lookup(&self, ctx: &BenchContext) -> Option<()> {
        for k in &ctx.lookup_keys { black_box(sorted_vec_get(&self.sorted, k)); }
        Some(())
    }

    fn bench_fwd_iter(&self) -> Option<()> {
        for (k, v) in self.sorted.iter() { black_box(k); black_box(v); }
        Some(())
    }

    fn bench_memory(&self, keys: &[Vec<u8>]) -> Option<f64> {
        let before = read_allocated();
        let s = build_sorted_vec(keys);
        let bytes = read_allocated() - before;
        drop(s);
        Some(bytes as f64 / keys.len() as f64)
    }
}

// ── LinkedList ──────────────────────────────────────────────────────────

pub(crate) struct LinkedListBench {
    list: LinkedList<(Vec<u8>, usize)>,
}

impl LinkedListBench {
    pub(crate) fn new() -> Self { Self { list: LinkedList::new() } }
}

impl Benchable for LinkedListBench {
    fn build(&mut self, keys: &[Vec<u8>], _ctx: &BenchContext) {
        self.list = keys.iter().enumerate().map(|(i, k)| (k.clone(), i)).collect();
    }

    fn bench_insert(&self, keys: &[Vec<u8>]) -> Option<()> {
        let mut list: LinkedList<(Vec<u8>, usize)> = LinkedList::new();
        for (i, k) in keys.iter().enumerate() { list.push_back((k.clone(), i)); }
        black_box(&list);
        Some(())
    }

    fn bench_lookup(&self, ctx: &BenchContext) -> Option<()> {
        for k in &ctx.lookup_keys { black_box(self.list.iter().find(|(key, _)| key == k)); }
        Some(())
    }

    fn bench_fwd_iter(&self) -> Option<()> {
        for (k, v) in self.list.iter() { black_box(k); black_box(v); }
        Some(())
    }

    fn bench_rev_iter(&self) -> Option<()> {
        for (k, v) in self.list.iter().rev() { black_box(k); black_box(v); }
        Some(())
    }

    fn bench_memory(&self, keys: &[Vec<u8>]) -> Option<f64> {
        let before = read_allocated();
        let mut m: LinkedList<(Vec<u8>, usize)> = LinkedList::new();
        for (i, k) in keys.iter().enumerate() { m.push_back((k.clone(), i)); }
        let bytes = read_allocated() - before;
        drop(m);
        Some(bytes as f64 / keys.len() as f64)
    }
}
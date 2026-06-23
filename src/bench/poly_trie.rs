use std::hint::black_box;

use tiny_trie::PolyTrie;

use super::{Benchable, BenchContext, KeyDomain, read_allocated};

pub(crate) struct PolyTrieBench {
    trie: PolyTrie<usize>,
}

impl PolyTrieBench {
    pub(crate) fn new() -> Self { Self { trie: PolyTrie::new() } }
}

impl Benchable for PolyTrieBench {
    fn key_domain(&self) -> KeyDomain { KeyDomain::Strings }

    fn build(&mut self, keys: &[Vec<u8>], _ctx: &BenchContext) {
        self.trie = PolyTrie::new();
        for (i, k) in keys.iter().enumerate() { self.trie.insert(k.clone(), i).unwrap(); }
    }

    fn bench_insert(&self, keys: &[Vec<u8>]) -> Option<()> {
        let mut m = PolyTrie::<usize>::new();
        for (i, k) in keys.iter().enumerate() { m.insert(k.clone(), i).unwrap(); }
        black_box(&m);
        Some(())
    }

    fn bench_optimize(&self, keys: &[Vec<u8>]) -> Option<()> {
        let mut m = PolyTrie::<usize>::new();
        for (i, k) in keys.iter().enumerate() { m.insert(k.clone(), i).unwrap(); }
        m.optimize();
        black_box(&m);
        Some(())
    }

    fn bench_memory(&self, keys: &[Vec<u8>]) -> Option<f64> {
        let before = read_allocated();
        let mut m: PolyTrie<usize> = PolyTrie::new();
        for (i, k) in keys.iter().enumerate() { m.insert(k.clone(), i).unwrap(); }
        let bytes = read_allocated() - before;
        drop(m);
        Some(bytes as f64 / keys.len() as f64)
    }

    bench_query_methods! {
        field: trie,
        lookup: get(null),
        fwd_iter: iter_kv,
        rev_iter: iter_kv,
    }
}

// ── PolyOpt ────────────────────────────────────────────────────────────

pub(crate) struct PolyOptBench {
    trie: PolyTrie<usize>,
}

impl PolyOptBench {
    pub(crate) fn new() -> Self { Self { trie: PolyTrie::new() } }
}

impl Benchable for PolyOptBench {
    fn key_domain(&self) -> KeyDomain { KeyDomain::Strings }

    fn build(&mut self, keys: &[Vec<u8>], _ctx: &BenchContext) {
        self.trie = PolyTrie::new();
        for (i, k) in keys.iter().enumerate() { self.trie.insert(k.clone(), i).unwrap(); }
        self.trie.optimize();
    }

    fn bench_optimize(&self, keys: &[Vec<u8>]) -> Option<()> {
        let mut m = PolyTrie::<usize>::new();
        for (i, k) in keys.iter().enumerate() { m.insert(k.clone(), i).unwrap(); }
        m.optimize();
        black_box(&m);
        Some(())
    }

    fn bench_memory(&self, keys: &[Vec<u8>]) -> Option<f64> {
        let before = read_allocated();
        let mut m: PolyTrie<usize> = PolyTrie::new();
        for (i, k) in keys.iter().enumerate() { m.insert(k.clone(), i).unwrap(); }
        m.optimize();
        let bytes = read_allocated() - before;
        drop(m);
        Some(bytes as f64 / keys.len() as f64)
    }

    bench_query_methods! {
        field: trie,
        lookup: get(null),
        fwd_iter: iter_kv,
        rev_iter: iter_kv,
    }
}
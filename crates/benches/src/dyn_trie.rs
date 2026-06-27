use std::hint::black_box;

use tiny_trie::DynTrie;

use super::{Benchable, BenchContext, read_allocated};

pub(crate) struct DynTrieBench {
    trie: DynTrie<usize>,
}

impl DynTrieBench {
    pub(crate) fn new() -> Self { Self { trie: DynTrie::new() } }
}

impl Benchable<Vec<u8>> for DynTrieBench {
    fn build(&mut self, keys: &[Vec<u8>], _ctx: &BenchContext) {
        self.trie = DynTrie::new();
        for (i, k) in keys.iter().enumerate() { self.trie.insert(k.clone(), i).unwrap(); }
    }

    fn bench_insert(&self, keys: &[Vec<u8>]) -> Option<()> {
        let mut m = DynTrie::new();
        for (i, k) in keys.iter().enumerate() { m.insert(k.clone(), i).unwrap(); }
        black_box(&m);
        Some(())
    }

    fn bench_memory(&self, keys: &[Vec<u8>]) -> Option<f64> {
        let before = read_allocated();
        let mut m: DynTrie<usize> = DynTrie::new();
        for (i, k) in keys.iter().enumerate() { m.insert(k.clone(), i).unwrap(); }
        let bytes = read_allocated() - before;
        drop(m);
        Some(bytes as f64 / keys.len() as f64)
    }

    bench_query_methods! {
        field: trie,
        ctx: BenchContext,
        lookup: get(lookup),
        fwd_iter: dyn_callback,
        rev_iter: dyn_callback,
    }
}

// ── DynTrieOpt ────────────────────────────────────────────────────────

pub(crate) struct DynTrieOptBench {
    trie: DynTrie<usize>,
}

impl DynTrieOptBench {
    pub(crate) fn new() -> Self { Self { trie: DynTrie::new() } }
}

impl Benchable<Vec<u8>> for DynTrieOptBench {
    fn build(&mut self, keys: &[Vec<u8>], _ctx: &BenchContext) {
        self.trie = DynTrie::new();
        for (i, k) in keys.iter().enumerate() { self.trie.insert(k.clone(), i).unwrap(); }
        self.trie.optimize();
    }

    fn bench_optimize(&self, keys: &[Vec<u8>]) -> Option<()> {
        let mut m = DynTrie::new();
        for (i, k) in keys.iter().enumerate() { m.insert(k.clone(), i).unwrap(); }
        m.optimize();
        black_box(&m);
        Some(())
    }

    fn bench_memory(&self, keys: &[Vec<u8>]) -> Option<f64> {
        let before = read_allocated();
        let mut m: DynTrie<usize> = DynTrie::new();
        for (i, k) in keys.iter().enumerate() { m.insert(k.clone(), i).unwrap(); }
        m.optimize();
        let bytes = read_allocated() - before;
        drop(m);
        Some(bytes as f64 / keys.len() as f64)
    }

    bench_query_methods! {
        field: trie,
        ctx: BenchContext,
        lookup: get(lookup),
        fwd_iter: dyn_callback,
        rev_iter: dyn_callback,
    }
}
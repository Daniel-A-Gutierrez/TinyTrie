use std::hint::black_box;

use tiny_trie::TinyTrieMap;

use super::{Benchable, BenchContext, NT, read_allocated};

pub(crate) struct NibbleTrieBench {
    trie: NT,
}

impl NibbleTrieBench {
    pub(crate) fn new() -> Self { Self { trie: NT::new() } }
}

impl Benchable<Vec<u8>> for NibbleTrieBench {
    fn build(&mut self, keys: &[Vec<u8>], _ctx: &BenchContext) {
        self.trie = NT::trie_new();
        for (i, k) in keys.iter().enumerate() { self.trie.trie_insert(k.clone(), i); }
    }

    fn bench_insert(&self, keys: &[Vec<u8>]) -> Option<()> {
        let mut m = NT::trie_new();
        for (i, k) in keys.iter().enumerate() { m.trie_insert(k.clone(), i); }
        black_box(&m);
        Some(())
    }

    fn bench_memory(&self, keys: &[Vec<u8>]) -> Option<f64> {
        let before = read_allocated();
        let mut m = NT::trie_new();
        for (i, k) in keys.iter().enumerate() { m.trie_insert(k.clone(), i); }
        let bytes = read_allocated() - before;
        drop(m);
        Some(bytes as f64 / keys.len() as f64)
    }

    bench_query_methods! {
        field: trie,
        ctx: BenchContext,
        lookup: trie_get(lookup),
        fwd_iter: trie_callback,
        rev_iter: trie_callback,
        index_iter: true,
    }
}

// ── NibbleOpt ────────────────────────────────────────────────────────

pub(crate) struct NibbleOptBench {
    trie: NT,
}

impl NibbleOptBench {
    pub(crate) fn new() -> Self { Self { trie: NT::new() } }
}

impl Benchable<Vec<u8>> for NibbleOptBench {
    fn build(&mut self, keys: &[Vec<u8>], _ctx: &BenchContext) {
        self.trie = NT::trie_new();
        for (i, k) in keys.iter().enumerate() { self.trie.trie_insert(k.clone(), i); }
        self.trie.trie_optimize();
    }

    fn bench_insert(&self, keys: &[Vec<u8>]) -> Option<()> {
        let mut m = NT::trie_new();
        for (i, k) in keys.iter().enumerate() { m.trie_insert(k.clone(), i); }
        black_box(&m);
        Some(())
    }

    fn bench_optimize(&self, keys: &[Vec<u8>]) -> Option<()> {
        let mut m = NT::trie_new();
        for (i, k) in keys.iter().enumerate() { m.trie_insert(k.clone(), i); }
        m.trie_optimize();
        black_box(&m);
        Some(())
    }

    fn bench_memory(&self, keys: &[Vec<u8>]) -> Option<f64> {
        let before = read_allocated();
        let mut m = NT::trie_new();
        for (i, k) in keys.iter().enumerate() { m.trie_insert(k.clone(), i); }
        m.trie_optimize();
        let bytes = read_allocated() - before;
        drop(m);
        Some(bytes as f64 / keys.len() as f64)
    }

    bench_query_methods! {
        field: trie,
        ctx: BenchContext,
        lookup: trie_get(lookup),
        fwd_iter: trie_callback,
        rev_iter: trie_callback,
        index_iter: true,
    }
}

// ── NibbleUnchecked ──────────────────────────────────────────────────

pub(crate) struct NibbleUncheckedBench {
    trie: NT,
}

impl NibbleUncheckedBench {
    pub(crate) fn new() -> Self { Self { trie: NT::new() } }
}

impl Benchable<Vec<u8>> for NibbleUncheckedBench {
    fn build(&mut self, keys: &[Vec<u8>], _ctx: &BenchContext) {
        self.trie = NT::trie_new();
        for (i, k) in keys.iter().enumerate() { self.trie.trie_insert(k.clone(), i); }
    }

    bench_query_methods! {
        field: trie,
        ctx: BenchContext,
        lookup: get_unchecked(hit),
        fwd_iter: none,
        rev_iter: none,
        unchecked: true,
    }
}

// ── NibbleOptUnchecked ────────────────────────────────────────────────

pub(crate) struct NibbleOptUncheckedBench {
    trie: NT,
}

impl NibbleOptUncheckedBench {
    pub(crate) fn new() -> Self { Self { trie: NT::new() } }
}

impl Benchable<Vec<u8>> for NibbleOptUncheckedBench {
    fn build(&mut self, keys: &[Vec<u8>], _ctx: &BenchContext) {
        self.trie = NT::trie_new();
        for (i, k) in keys.iter().enumerate() { self.trie.trie_insert(k.clone(), i); }
        self.trie.trie_optimize();
    }

    bench_query_methods! {
        field: trie,
        ctx: BenchContext,
        lookup: get_unchecked(hit),
        fwd_iter: none,
        rev_iter: none,
        unchecked: true,
    }
}
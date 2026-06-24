use std::hint::black_box;

use tiny_trie::PolyTrie;

use super::{Benchable, BenchContextNz, NonZeroBytes, read_allocated};

/// `PolyTrie` is a null-terminator trie (like `BitTrie`): `insert` rejects keys
/// containing `0x00` and appends its own terminator; `get` requires null-
/// terminated input (the macro's `get(null)` arm feeds it `ctx.lookup_keys_null`).
/// So its native key type is `NonZeroBytes` — it runs only in `0x00`-free modes
/// (Sequential/Lines/Words) and skips null-byte modes by construction
/// (`Bench::NonZero::skip_for`), no runtime domain check.
pub(crate) struct PolyTrieBench {
    trie: PolyTrie<usize>,
}

impl PolyTrieBench {
    pub(crate) fn new() -> Self { Self { trie: PolyTrie::new() } }
}

impl Benchable<NonZeroBytes> for PolyTrieBench {
    fn build(&mut self, keys: &[NonZeroBytes], _ctx: &BenchContextNz) {
        self.trie = PolyTrie::new();
        for (i, k) in keys.iter().enumerate() { self.trie.insert(k.to_vec(), i).unwrap(); }
    }

    fn bench_insert(&self, keys: &[NonZeroBytes]) -> Option<()> {
        let mut m = PolyTrie::<usize>::new();
        for (i, k) in keys.iter().enumerate() { m.insert(k.to_vec(), i).unwrap(); }
        black_box(&m);
        Some(())
    }

    fn bench_optimize(&self, keys: &[NonZeroBytes]) -> Option<()> {
        let mut m = PolyTrie::<usize>::new();
        for (i, k) in keys.iter().enumerate() { m.insert(k.to_vec(), i).unwrap(); }
        m.optimize();
        black_box(&m);
        Some(())
    }

    fn bench_memory(&self, keys: &[NonZeroBytes]) -> Option<f64> {
        let before = read_allocated();
        let mut m = PolyTrie::<usize>::new();
        for (i, k) in keys.iter().enumerate() { m.insert(k.to_vec(), i).unwrap(); }
        let bytes = read_allocated() - before;
        drop(m);
        Some(bytes as f64 / keys.len() as f64)
    }

    bench_query_methods! {
        field: trie,
        ctx: BenchContextNz,
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

impl Benchable<NonZeroBytes> for PolyOptBench {
    fn build(&mut self, keys: &[NonZeroBytes], _ctx: &BenchContextNz) {
        self.trie = PolyTrie::new();
        for (i, k) in keys.iter().enumerate() { self.trie.insert(k.to_vec(), i).unwrap(); }
        self.trie.optimize();
    }

    fn bench_optimize(&self, keys: &[NonZeroBytes]) -> Option<()> {
        let mut m = PolyTrie::<usize>::new();
        for (i, k) in keys.iter().enumerate() { m.insert(k.to_vec(), i).unwrap(); }
        m.optimize();
        black_box(&m);
        Some(())
    }

    fn bench_memory(&self, keys: &[NonZeroBytes]) -> Option<f64> {
        let before = read_allocated();
        let mut m = PolyTrie::<usize>::new();
        for (i, k) in keys.iter().enumerate() { m.insert(k.to_vec(), i).unwrap(); }
        m.optimize();
        let bytes = read_allocated() - before;
        drop(m);
        Some(bytes as f64 / keys.len() as f64)
    }

    bench_query_methods! {
        field: trie,
        ctx: BenchContextNz,
        lookup: get(null),
        fwd_iter: iter_kv,
        rev_iter: iter_kv,
    }
}
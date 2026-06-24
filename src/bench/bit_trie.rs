use std::hint::black_box;

use tiny_trie::BitTrie;

use super::{Benchable, BenchContextNz, NonZeroBytes, read_allocated};

/// `BitTrie` is a null-terminator trie: `insert` rejects keys containing `0x00`
/// and appends its own terminator; `get` requires null-terminated input (the
/// macro's `get(null)` arm feeds it `ctx.lookup_keys_null`). So the contestant's
/// native key type is `NonZeroBytes` — the harness only produces these in
/// `0x00`-free modes (Sequential/Lines/Words), and this contestant skips
/// null-byte modes by construction (no keys to build on). Previously it was
/// mis-declared `Any` and silently dropped every `0x00`-containing key in
/// Random/u64 modes, building a tree with fewer than `size` entries.
pub(crate) struct BitTrieBench {
    trie: BitTrie<Vec<u8>, usize>,
}

impl BitTrieBench {
    pub(crate) fn new() -> Self { Self { trie: BitTrie::new() } }
}

impl Benchable<NonZeroBytes> for BitTrieBench {
    fn build(&mut self, keys: &[NonZeroBytes], _ctx: &BenchContextNz) {
        self.trie = BitTrie::new();
        for (i, k) in keys.iter().enumerate() { self.trie.insert(k.to_vec(), i).unwrap(); }
    }

    fn bench_insert(&self, keys: &[NonZeroBytes]) -> Option<()> {
        let mut m = BitTrie::<Vec<u8>, usize>::new();
        for (i, k) in keys.iter().enumerate() { m.insert(k.to_vec(), i).unwrap(); }
        black_box(&m);
        Some(())
    }

    fn bench_memory(&self, keys: &[NonZeroBytes]) -> Option<f64> {
        let before = read_allocated();
        let mut m: BitTrie<Vec<u8>, usize> = BitTrie::new();
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
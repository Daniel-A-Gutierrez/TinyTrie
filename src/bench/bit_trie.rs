use std::hint::black_box;

use tiny_trie::{BitTrie, TinyTrieMap};

use super::{Benchable, BenchContext, read_allocated};

pub(crate) struct BitTrieBench {
    trie: BitTrie<Vec<u8>, usize>,
}

impl BitTrieBench {
    pub(crate) fn new() -> Self { Self { trie: BitTrie::new() } }
}

impl Benchable for BitTrieBench {
    fn build(&mut self, keys: &[Vec<u8>], _ctx: &BenchContext) {
        self.trie = BitTrie::new();
        for (i, k) in keys.iter().enumerate() { self.trie.insert(k.clone(), i).unwrap(); }
    }

    fn bench_insert(&self, keys: &[Vec<u8>]) -> Option<()> {
        let mut m = BitTrie::<Vec<u8>, usize>::new();
        for (i, k) in keys.iter().enumerate() { m.insert(k.clone(), i).unwrap(); }
        black_box(&m);
        Some(())
    }

    fn bench_memory(&self, keys: &[Vec<u8>]) -> Option<f64> {
        let before = read_allocated();
        let mut m: BitTrie<Vec<u8>, usize> = BitTrie::new();
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
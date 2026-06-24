use std::hint::black_box;

use tiny_trie::TinyTrieMap;

use super::{Benchable, BenchContext, NT, NT2, NT4, convert_stak1_to, read_allocated};

pub(crate) struct StackedTrie2Bench {
    trie: NT2,
}

impl StackedTrie2Bench {
    pub(crate) fn new() -> Self { Self { trie: NT2::new() } }
}

impl Benchable<Vec<u8>> for StackedTrie2Bench {
    fn build(&mut self, keys: &[Vec<u8>], _ctx: &BenchContext) {
        let mut base = NT::trie_new();
        for (i, k) in keys.iter().enumerate() { base.trie_insert(k.clone(), i); }
        self.trie = convert_stak1_to::<2>(&base);
        self.trie.optimize();
    }

    fn bench_optimize(&self, keys: &[Vec<u8>]) -> Option<()> {
        let mut m = NT::trie_new();
        for (i, k) in keys.iter().enumerate() { m.trie_insert(k.clone(), i); }
        let mut m2 = convert_stak1_to::<2>(&m);
        m2.optimize();
        black_box(&m2);
        Some(())
    }

    fn bench_memory(&self, keys: &[Vec<u8>]) -> Option<f64> {
        let before = read_allocated();
        let mut m = NT::trie_new();
        for (i, k) in keys.iter().enumerate() { m.trie_insert(k.clone(), i); }
        let mut m2 = convert_stak1_to::<2>(&m);
        drop(m);
        m2.optimize();
        let bytes = read_allocated() - before;
        drop(m2);
        Some(bytes as f64 / keys.len() as f64)
    }

    bench_query_methods! {
        field: trie,
        ctx: BenchContext,
        lookup: get(lookup),
        fwd_iter: iter_kv_no_current,
        rev_iter: iter_kv_no_current,
        index_iter: true,
    }
}

pub(crate) struct StackedTrie4Bench {
    trie: NT4,
}

impl StackedTrie4Bench {
    pub(crate) fn new() -> Self { Self { trie: NT4::new() } }
}

impl Benchable<Vec<u8>> for StackedTrie4Bench {
    fn build(&mut self, keys: &[Vec<u8>], _ctx: &BenchContext) {
        let mut base = NT::trie_new();
        for (i, k) in keys.iter().enumerate() { base.trie_insert(k.clone(), i); }
        self.trie = convert_stak1_to::<4>(&base);
        self.trie.optimize();
    }

    fn bench_optimize(&self, keys: &[Vec<u8>]) -> Option<()> {
        let mut m = NT::trie_new();
        for (i, k) in keys.iter().enumerate() { m.trie_insert(k.clone(), i); }
        let mut m4 = convert_stak1_to::<4>(&m);
        m4.optimize();
        black_box(&m4);
        Some(())
    }

    fn bench_memory(&self, keys: &[Vec<u8>]) -> Option<f64> {
        let before = read_allocated();
        let mut m = NT::trie_new();
        for (i, k) in keys.iter().enumerate() { m.trie_insert(k.clone(), i); }
        let mut m4 = convert_stak1_to::<4>(&m);
        drop(m);
        m4.optimize();
        let bytes = read_allocated() - before;
        drop(m4);
        Some(bytes as f64 / keys.len() as f64)
    }

    bench_query_methods! {
        field: trie,
        ctx: BenchContext,
        lookup: get(lookup),
        fwd_iter: iter_kv_no_current,
        rev_iter: iter_kv_no_current,
        index_iter: true,
    }
}
use std::collections::HashSet;
use std::hint::black_box;

use tiny_trie::{FixedLenNibbleTrie, TinyTrieMap};

use super::{Benchable, BenchContext, truncate_key, max_key_len, read_allocated};

pub(crate) struct FixedLenBench {
    trie: FixedLenNibbleTrie<usize, u32>,
}

impl FixedLenBench {
    pub(crate) fn new() -> Self { Self { trie: FixedLenNibbleTrie::new(1) } }
}

impl Benchable for FixedLenBench {
    fn build(&mut self, keys: &[Vec<u8>], _ctx: &BenchContext) {
        let max_len = max_key_len(keys);
        self.trie = FixedLenNibbleTrie::new(max_len);
        let mut seen: HashSet<Vec<u8>> = HashSet::new();
        for (i, k) in keys.iter().enumerate() {
            let tk = truncate_key(k);
            if seen.insert(tk.clone()) {
                self.trie.insert(tk, i).unwrap();
            }
        }
    }

    fn bench_insert(&self, keys: &[Vec<u8>]) -> Option<()> {
        let max_len = max_key_len(keys);
        let mut m = FixedLenNibbleTrie::<usize, u32>::new(max_len);
        let mut seen: HashSet<Vec<u8>> = HashSet::new();
        for (i, k) in keys.iter().enumerate() {
            let tk = truncate_key(k);
            if seen.insert(tk.clone()) {
                m.insert(tk, i).unwrap();
            }
        }
        black_box(&m);
        Some(())
    }

    fn bench_memory(&self, keys: &[Vec<u8>]) -> Option<f64> {
        let max_len = max_key_len(keys);
        let before = read_allocated();
        let mut m = FixedLenNibbleTrie::<usize, u32>::new(max_len);
        let mut seen: HashSet<Vec<u8>> = HashSet::new();
        for (i, k) in keys.iter().enumerate() {
            let tk = truncate_key(k);
            if seen.insert(tk.clone()) {
                m.insert(tk, i).unwrap();
            }
        }
        let bytes = read_allocated() - before;
        drop(m);
        Some(bytes as f64 / keys.len() as f64)
    }

    bench_query_methods! {
        field: trie,
        lookup: get(truncated),
        fwd_iter: iter_kv,
        rev_iter: iter_kv,
        index_iter: true,
    }
}

// ── FixedLenOpt ────────────────────────────────────────────────────────

pub(crate) struct FixedLenOptBench {
    trie: FixedLenNibbleTrie<usize, u32>,
}

impl FixedLenOptBench {
    pub(crate) fn new() -> Self { Self { trie: FixedLenNibbleTrie::new(1) } }
}

impl Benchable for FixedLenOptBench {
    fn build(&mut self, keys: &[Vec<u8>], _ctx: &BenchContext) {
        let max_len = max_key_len(keys);
        self.trie = FixedLenNibbleTrie::new(max_len);
        let mut seen: HashSet<Vec<u8>> = HashSet::new();
        for (i, k) in keys.iter().enumerate() {
            let tk = truncate_key(k);
            if seen.insert(tk.clone()) {
                self.trie.insert(tk, i).unwrap();
            }
        }
        self.trie.optimize();
    }

    fn bench_optimize(&self, keys: &[Vec<u8>]) -> Option<()> {
        let max_len = max_key_len(keys);
        let mut m = FixedLenNibbleTrie::<usize, u32>::new(max_len);
        let mut seen: HashSet<Vec<u8>> = HashSet::new();
        for (i, k) in keys.iter().enumerate() {
            let tk = truncate_key(k);
            if seen.insert(tk.clone()) {
                m.insert(tk, i).unwrap();
            }
        }
        m.optimize();
        black_box(&m);
        Some(())
    }

    fn bench_memory(&self, keys: &[Vec<u8>]) -> Option<f64> {
        let max_len = max_key_len(keys);
        let before = read_allocated();
        let mut m = FixedLenNibbleTrie::<usize, u32>::new(max_len);
        let mut seen: HashSet<Vec<u8>> = HashSet::new();
        for (i, k) in keys.iter().enumerate() {
            let tk = truncate_key(k);
            if seen.insert(tk.clone()) {
                m.insert(tk, i).unwrap();
            }
        }
        m.optimize();
        let bytes = read_allocated() - before;
        drop(m);
        Some(bytes as f64 / keys.len() as f64)
    }

    bench_query_methods! {
        field: trie,
        lookup: get(truncated),
        fwd_iter: iter_kv,
        rev_iter: iter_kv,
        index_iter: true,
    }
}
use std::hint::black_box;

use btrees::{CTree, SearchStrategy, StoredKey, TreeKey};

use super::{Benchable, BenchCtx, read_allocated};

// ── IntBTreeKey adapter ───────────────────────────────────────────────────
//
// Unifies the two IntBTree key forms behind one generic bench struct.
// Fixed keys (u64) use SIMD search; variable keys (Vec<u8>) use KeyRef
// with inline short keys and linear scan through key_buf for longer keys.

pub(crate) trait IntBTreeBenchKey: TreeKey + SearchStrategy + Clone + Ord + 'static {}
impl IntBTreeBenchKey for u64 {}
impl IntBTreeBenchKey for Vec<u8> {}

// ── IntBTreeBenchGen ─────────────────────────────────────────────────────
//
// One generic contestant over both IntBTree key forms. `OPT` selects the
// `optimize`-after-build variant. `max_key: Option<K>` tracks the largest
// inserted key in *harness-key* form so reverse iteration can seed
// `cursor_at` via `as_needle`, and an empty tree is handled without
// panicking (`None` → the rev-iter bench returns early).
//
// Monomorphization: `K` is never erased here — `IntBTreeBenchGen<u64, _>` holds a
// concrete `CTree<u64, …>`, so `get`/`find_position`/`find_upper_bound` inline
// the SIMD path. The `dyn Benchable<u64>` vtable in the harness only erases the
// *contestant type*, not `K`.

pub(crate) struct IntBTreeBenchGen<K: IntBTreeBenchKey, V, PTR, const N: usize, const NP1: usize, const OPT: bool>
where
    K: TreeKey,
    PTR: btrees::TrieIndex,
{
    tree: CTree<K, V, PTR, N, NP1>,
    max_key: Option<K>,
}

impl<K, V, PTR, const N: usize, const NP1: usize, const OPT: bool>
    IntBTreeBenchGen<K, V, PTR, N, NP1, OPT>
where
    K: IntBTreeBenchKey + TreeKey + SearchStrategy + Clone + Ord + 'static,
    K::Stored: StoredKey,
    PTR: btrees::TrieIndex,
    V: From<usize>,
    [(); N]: ,
    [(); NP1]: ,
{
    pub(crate) fn new() -> Self {
        Self { tree: CTree::new(), max_key: None }
    }

    /// Insert all keys, tracking the largest for reverse iteration.
    fn build_tree(keys: &[K]) -> (CTree<K, V, PTR, N, NP1>, Option<K>)
    where
        K: Clone,
        V: From<usize>,
    {
        let mut tree = CTree::new();
        let mut max_key: Option<K> = None;
        for (i, k) in keys.iter().enumerate() {
            let _ = tree.insert(k.clone(), V::from(i));
            if max_key.as_ref().map_or(true, |m| k > m) {
                max_key = Some(k.clone());
            }
        }
        (tree, max_key)
    }
}

// Benchable impl for u64 values (the common case)
impl<K, PTR, const N: usize, const NP1: usize, const OPT: bool>
    Benchable<K> for IntBTreeBenchGen<K, usize, PTR, N, NP1, OPT>
where
    K: IntBTreeBenchKey + TreeKey + SearchStrategy + Clone + Ord + 'static,
    K::Stored: StoredKey,
    PTR: btrees::TrieIndex,
    [(); N]: ,
    [(); NP1]: ,
{
    fn build(&mut self, keys: &[K], _ctx: &BenchCtx<K>) {
        let (mut tree, max_key) = Self::build_tree(keys);
        tree.compact();
        if OPT { tree.optimize(); }
        self.tree = tree;
        self.max_key = max_key;
    }

    fn bench_insert(&self, keys: &[K]) -> Option<()> {
        let (tree, _) = Self::build_tree(keys);
        black_box(&tree);
        Some(())
    }

    fn bench_lookup(&self, ctx: &BenchCtx<K>) -> Option<()> {
        for k in &ctx.lookup_keys {
            black_box(self.tree.get(k.as_needle()));
        }
        Some(())
    }

    fn bench_fwd_iter(&self) -> Option<()> {
        let mut it = self.tree.get_cursor();
        while let Some((k, v)) = it.current() {
            black_box(k);
            black_box(v);
            if it.next().is_none() {
                break;
            }
        }
        Some(())
    }

    fn bench_rev_iter(&self) -> Option<()> {
        let max = self.max_key.as_ref()?.as_needle();
        let mut it = self.tree.cursor_at(max);
        while let Some((k, v)) = it.current() {
            black_box(k);
            black_box(v);
            if it.prev().is_none() {
                break;
            }
        }
        Some(())
    }

    fn bench_optimize(&self, keys: &[K]) -> Option<()> {
        if !OPT { return None; }
        let (mut tree, _) = Self::build_tree(keys);
        tree.compact();
        tree.optimize();
        black_box(&tree);
        Some(())
    }

    fn bench_memory(&self, keys: &[K]) -> Option<f64> {
        let before = read_allocated();
        let (mut tree, _) = Self::build_tree(keys);
        tree.compact();
        if OPT { tree.optimize(); }
        let n = tree.len();
        let bytes = read_allocated() - before;
        drop(tree);
        Some(bytes as f64 / n as f64)
    }
}

// ── Contestant aliases ─────────────────────────────────────────────────

pub(crate) type IntBTreeBench = IntBTreeBenchGen<Vec<u8>, usize, u32, 8, 9, false>;
/// `IntBTreeBench` + `optimize` after build (arena contiguity for iteration).
pub(crate) type IntBTreeOptBench = IntBTreeBenchGen<Vec<u8>, usize, u32, 4, 5, true>;

// ── StrBTree contestant ──────────────────────────────────────────────────
//
// Variable-length key CTree using KeySlots (inline key storage
// with branch-free sequential scan). Benchmarked against the existing
// IntBTreeBench (which uses KeyRef with inline/buf branching).

use btrees::LengthType;
use btrees::StrBTree;

pub(crate) struct StrBTreeBench {
    tree: StrBTree<Vec<u8>, usize, u32, u8, 8, 9>,
    max_key: Option<Vec<u8>>,
}

impl StrBTreeBench {
    pub(crate) fn new() -> Self {
        Self { tree: StrBTree::new(), max_key: None }
    }

    /// Insert keys that fit within the length type's maximum, skipping those
    /// that are too long. Returns the actual count inserted and warns on stderr
    /// if any keys were rejected.
    fn build_tree(keys: &[Vec<u8>]) -> (StrBTree<Vec<u8>, usize, u32, u8, 8, 9>, Option<Vec<u8>>, usize) {
        let max_len = <u8 as LengthType>::max();
        let mut tree = StrBTree::new();
        let mut max_key: Option<Vec<u8>> = None;
        let mut rejected = 0usize;
        for (i, k) in keys.iter().enumerate() {
            if k.len() > max_len {
                rejected += 1;
                continue;
            }
            if tree.insert(k.clone(), i).is_ok() {
                if max_key.as_ref().map_or(true, |m| k > m) {
                    max_key = Some(k.clone());
                }
            }
        }
        if rejected > 0 {
            eprintln!(
                "StrBTree: rejected {}/{} keys (exceed max length of {} bytes)",
                rejected, keys.len(), max_len
            );
        }
        let n = tree.len();
        (tree, max_key, n)
    }
}

impl Benchable<Vec<u8>> for StrBTreeBench {
    fn build(&mut self, keys: &[Vec<u8>], _ctx: &BenchCtx<Vec<u8>>) {
        let (mut tree, max_key, _) = Self::build_tree(keys);
        tree.compact();
        self.tree = tree;
        self.max_key = max_key;
    }

    fn bench_insert(&self, keys: &[Vec<u8>]) -> Option<()> {
        let (tree, _, _) = Self::build_tree(keys);
        black_box(&tree);
        Some(())
    }

    fn bench_lookup(&self, ctx: &BenchCtx<Vec<u8>>) -> Option<()> {
        for k in &ctx.lookup_keys {
            black_box(self.tree.get(k));
        }
        Some(())
    }

    fn bench_fwd_iter(&self) -> Option<()> {
        let mut it = self.tree.get_cursor();
        while let Some((k, v)) = it.current() {
            black_box(k);
            black_box(v);
            if it.next().is_none() {
                break;
            }
        }
        Some(())
    }

    fn bench_rev_iter(&self) -> Option<()> {
        let max = self.max_key.as_ref()?;
        let mut it = self.tree.cursor_at(max);
        while let Some((k, v)) = it.current() {
            black_box(k);
            black_box(v);
            if it.prev().is_none() {
                break;
            }
        }
        Some(())
    }

    fn bench_optimize(&self, _keys: &[Vec<u8>]) -> Option<()> {
        None // StrBTree optimize is a no-op for now
    }

    fn bench_memory(&self, keys: &[Vec<u8>]) -> Option<f64> {
        let before = read_allocated();
        let (mut tree, _, n) = Self::build_tree(keys);
        tree.compact();
        let bytes = read_allocated() - before;
        drop(tree);
        Some(bytes as f64 / n as f64)
    }
}
use std::hint::black_box;

use tiny_trie::tiny_btree::{CTree, NoPreview, StoredKey, TreeKey};

use super::{Benchable, BenchCtx, read_allocated};

// ── CTreeKey adapter ───────────────────────────────────────────────────
//
// Unifies the two CTree key forms behind one generic bench struct. The new
// CTree signature is `CTree<K, V, PTR, N, NP1, P>` where `K: TreeKey + Preview<P>`.
// For fixed keys (`u64`), `P = NoPreview` (default). For variable keys (`Vec<u8>`),
// `P = u64` (u64 preview for SIMD search).
//
// The bench harness key `K` maps directly to CTree's `K`. For `u64` keys,
// `K = u64` and `P = NoPreview`. For `Vec<u8>` keys, `K = Vec<u8>` and `P = u64`.

pub(crate) trait CTreeBenchKey: TreeKey + Clone + Ord + 'static {
    /// Preview type for CTree's `P` parameter.
    type Preview: Copy + Eq + Ord;
}

impl CTreeBenchKey for u64 {
    type Preview = NoPreview;
}

impl CTreeBenchKey for Vec<u8> {
    type Preview = u64;
}

// ── CTreeBenchGen ─────────────────────────────────────────────────────
//
// One generic contestant over both CTree key forms. `OPT` selects the
// `optimize`-after-build variant. `max_key: Option<K>` tracks the largest
// inserted key in *harness-key* form so reverse iteration can seed
// `cursor_at` via `as_needle`, and an empty tree is handled without
// panicking (`None` → the rev-iter bench returns early).
//
// Monomorphization: `K` is never erased here — `CTreeBenchGen<u64, _>` holds a
// concrete `CTree<u64, …>`, so `get`/`find_position`/`find_upper_bound` inline
// the SIMD path. The `dyn Benchable<u64>` vtable in the harness only erases the
// *contestant type*, not `K`.

pub(crate) struct CTreeBenchGen<K: CTreeBenchKey, V, PTR, const N: usize, const NP1: usize, const OPT: bool, P = <K as CTreeBenchKey>::Preview>
where
    K: TreeKey + tiny_trie::tiny_btree::Preview<P>,
    P: Copy + Eq + Ord,
    PTR: tiny_trie::tiny_btree::TrieIndex,
{
    tree: CTree<K, V, PTR, N, NP1, P>,
    max_key: Option<K>,
}

// Fixed-key bench: CTree<u64, usize, u32, 4, 5, NoPreview>
impl<K, V, PTR, const N: usize, const NP1: usize, const OPT: bool, P>
    CTreeBenchGen<K, V, PTR, N, NP1, OPT, P>
where
    K: CTreeBenchKey + TreeKey + tiny_trie::tiny_btree::Preview<P> + Clone + Ord + 'static,
    K::Stored: StoredKey,
    P: Copy + Eq + Ord,
    PTR: tiny_trie::tiny_btree::TrieIndex,
    V: From<usize>,
    K: tiny_trie::tiny_btree::SearchStrategy<P>,
    [(); N]: ,
    [(); NP1]: ,
{
    pub(crate) fn new() -> Self {
        Self { tree: CTree::new(), max_key: None }
    }

    /// Insert every key into a fresh tree, tracking the largest harness key.
    fn build_tree(keys: &[K]) -> (CTree<K, V, PTR, N, NP1, P>, Option<K>)
    where
        K: Clone,
        V: From<usize>,
    {
        let mut tree = CTree::new();
        let mut max_key: Option<K> = None;
        for (i, k) in keys.iter().enumerate() {
            if tree.insert(k.clone(), V::from(i)).is_ok() && max_key.as_ref().map_or(true, |m| k > m) {
                max_key = Some(k.clone());
            }
        }
        (tree, max_key)
    }
}

// Benchable impl for u64 values (the common case)
impl<K, PTR, const N: usize, const NP1: usize, const OPT: bool, P>
    Benchable<K> for CTreeBenchGen<K, usize, PTR, N, NP1, OPT, P>
where
    K: CTreeBenchKey + TreeKey + tiny_trie::tiny_btree::Preview<P> + Clone + Ord + 'static,
    K::Stored: StoredKey,
    P: Copy + Eq + Ord,
    PTR: tiny_trie::tiny_btree::TrieIndex,
    K: tiny_trie::tiny_btree::SearchStrategy<P>,
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
        let bytes = read_allocated() - before;
        drop(tree);
        Some(bytes as f64 / keys.len() as f64)
    }
}

// ── Contestant aliases ─────────────────────────────────────────────────
//
// Variable-length `Vec<u8>` CTree — preview SIMD + scalar fallback — runs
// in every non-`u64` mode (`Bench::Bytes` skips `RandomU64`/`SeqU64`, where
// the native `u64` SIMD CTree below takes over).
pub(crate) type CTreeBench = CTreeBenchGen<Vec<u8>, usize, u32, 4, 5, false, u64>;
/// `CTreeBench` + `optimize` after build (arena contiguity for iteration).
pub(crate) type CTreeOptBench = CTreeBenchGen<Vec<u8>, usize, u32, 4, 5, true, u64>;

// Fixed-width `u64` CTree — SIMD `find_position`/`find_upper_bound` path —
// `RandomU64`/`SeqU64` modes only (`Bench::U64` carries the fixed-width skip).
pub(crate) type CTreeFixedBench = CTreeBenchGen<u64, usize, u32, 4, 5, false, NoPreview>;
/// `CTreeFixedBench` + `optimize` after build.
pub(crate) type CTreeFixedOptBench = CTreeBenchGen<u64, usize, u32, 4, 5, true, NoPreview>;
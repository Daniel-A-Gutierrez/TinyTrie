use std::hint::black_box;

use tiny_trie::tiny_btree::{CTree, StoredKey};

use super::{Benchable, BenchCtx, read_allocated};

// ── CTreeKey adapter ───────────────────────────────────────────────────
//
// Unifies the two `CTree` key forms behind one generic bench struct. The bench
// harness key `K` maps to CTree's stored key `Self::Stored` (the sealed
// `StoredKey` form: `Box<[u8]>` for variable-length, `u64` for fixed-width) and
// to the lookup needle. CTree's own `Borrow`-based `get`/`cursor_at` seam
// already accepts the needle borrow, so the bench bodies are byte-identical
// across both forms — only the conversion, the stored type, and the key
// domain differ, which this trait carries.
//
// `StoredKey` is `pub` (only its `Sealed` marker is private), so the bench can
// *name* it as a bound and *use* the existing `Box<[u8]>`/`u64` impls — it just
// can't impl `StoredKey` for new types, which it has no need to.
pub(crate) trait CTreeKey: Clone + Ord + 'static {
    /// CTree's owned stored key: `Box<[u8]>` (variable) or `u64` (fixed).
    /// `Default` seeds `max_key` for the empty-tree case (cursor → start).
    type Stored: StoredKey + Default;
    /// Harness key → CTree's owned stored key (`Box::from(k.as_slice())` | `*k`).
    fn into_stored(&self) -> Self::Stored;
    /// Harness key → lookup needle borrow (`&[u8]` | `&u64`).
    fn as_needle(&self) -> &<Self::Stored as StoredKey>::Needle;
}

impl CTreeKey for Vec<u8> {
    type Stored = Box<[u8]>;
    #[inline] fn into_stored(&self) -> Box<[u8]> { Box::from(self.as_slice()) }
    #[inline] fn as_needle(&self) -> &[u8] { self.as_slice() }
}

impl CTreeKey for u64 {
    type Stored = u64;
    #[inline] fn into_stored(&self) -> u64 { *self }
    #[inline] fn as_needle(&self) -> &u64 { self }
}

// ── CTreeBenchGen ─────────────────────────────────────────────────────
//
// One generic contestant over both `CTree` key forms. `OPT` selects the
// `optimize`-after-build variant. `max_key: Option<K>` tracks the largest
// inserted key in *harness-key* form (not stored form) so reverse iteration
// can seed `cursor_at` via `as_needle`, and an empty tree is handled without
// panicking (`None` → the rev-iter bench returns early).
//
// Monomorphization: `K` is never erased here — `CTreeBenchGen<u64, _>` holds a
// concrete `CTree<u64, …>`, so `get`/`find_position`/`find_upper_bound` inline
// the SIMD path. The `dyn Benchable<u64>` vtable in the harness only erases the
// *contestant type*, not `K`.
//
// Alloc behavior matches the old hand-written contestants: per insert, one
// `into_stored` conversion (consumed by `insert`) plus, only when this key is a
// new maximum, one `K::clone` for `max_key` — no per-insert clone of the stored
// key.

type Tree<K> = CTree<<K as CTreeKey>::Stored, usize, u32, 4, 5>;

pub(crate) struct CTreeBenchGen<K: CTreeKey, const OPT: bool> {
    tree: Tree<K>,
    max_key: Option<K>,
}

impl<K: CTreeKey, const OPT: bool> CTreeBenchGen<K, OPT> {
    pub(crate) fn new() -> Self {
        Self { tree: Tree::<K>::new(), max_key: None }
    }

    /// Insert every key into a fresh tree, tracking the largest harness key.
    ///
    /// `CTree::insert` returns `Err` on a duplicate key. The generated key modes
    /// (`random`, `sequential`, the `u64` modes) are already deduplicated by
    /// `keygen`; only `Words`/`Lines` corpus modes may carry duplicates, and for
    /// those the no-op `Err` path is the only cost. There is deliberately no
    /// `HashSet` dedup here: it would allocate a transient structure that
    /// contaminates `bench_memory`'s `read_allocated` delta and penalizes
    /// `bench_insert` with per-key hashing the tree doesn't need.
    fn build_tree(keys: &[K]) -> (Tree<K>, Option<K>) {
        let mut tree = Tree::<K>::new();
        let mut max_key: Option<K> = None;
        for (i, k) in keys.iter().enumerate() {
            let stored = k.into_stored();
            if tree.insert(stored, i).is_ok() && max_key.as_ref().map_or(true, |m| k > m) {
                max_key = Some(k.clone());
            }
        }
        (tree, max_key)
    }
}

impl<K: CTreeKey, const OPT: bool> Benchable<K> for CTreeBenchGen<K, OPT> {
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
        // Position at the largest key, then walk backward. `None` = empty tree
        // (no keys built) → nothing to iterate.
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
        // Measure the tree's steady-state footprint, not its transient growth
        // capacity: `compact` reclaims the `Vec` spare capacity.
        let bytes = read_allocated() - before;
        drop(tree);
        Some(bytes as f64 / keys.len() as f64)
    }
}

// ── Contestant aliases ─────────────────────────────────────────────────
//
// Variable-length `Box<[u8]>` CTree — scalar binary-search path — runs in every
// non-`u64` mode (`Bench::Bytes` skips `RandomU64`/`SeqU64`, where the native
// `u64` SIMD CTree below takes over).
pub(crate) type CTreeBench = CTreeBenchGen<Vec<u8>, false>;
/// `CTreeBench` + `optimize` after build (arena contiguity for iteration).
pub(crate) type CTreeOptBench = CTreeBenchGen<Vec<u8>, true>;

// Fixed-width `u64` CTree — SIMD `find_position`/`find_upper_bound` path —
// `RandomU64`/`SeqU64` modes only (`Bench::U64` carries the fixed-width skip).
// The harness hands it native `&[u64]` keys and `BenchCtx<u64>` lookup keys: no
// `Vec<u8>`→`u64` decode per op, SIMD inlines.
pub(crate) type CTreeFixedBench = CTreeBenchGen<u64, false>;
/// `CTreeFixedBench` + `optimize` after build.
pub(crate) type CTreeFixedOptBench = CTreeBenchGen<u64, true>;
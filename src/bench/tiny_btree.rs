use std::hint::black_box;

use tiny_trie::tiny_btree::CTree;

use super::{Benchable, BenchContext, KeyDomain, read_allocated};

/// B+ tree contestant.
///
/// `CTree` exposes a single unified interface over its key type; the bench
/// uses one instantiation — the variable-length form `CTree<Box<[u8]>>` — for
/// every key mode. There is no fixed/variable split and no head-to-head
/// between the two forms: the byte keys the harness produces go straight in,
/// and the varlen comparison path (leaf search, separator search, the final
/// equality check) is plain scalar element-wise `Ord`/`PartialEq` — the SIMD
/// `cmp_slice`/`eq_slice` variant measured no faster and was removed.
type Tree = CTree<Box<[u8]>, usize, u32, 4, 5>;

pub(crate) struct CTreeBench {
    tree: Tree,
    /// Lexicographically largest inserted key, used to seed reverse iteration.
    max_key: Vec<u8>,
}

impl CTreeBench {
    pub(crate) fn new() -> Self {
        Self { tree: Tree::new(), max_key: Vec::new() }
    }

    /// Insert every key into a fresh tree, tracking the lexicographic max.
    ///
    /// `CTree::insert` returns `Err` on a duplicate key. The generated key
    /// modes (`random`, `sequential`, the `u64` modes) are already
    /// deduplicated by `keygen`; only `Words`/`Lines` corpus modes may carry
    /// duplicates, and for those the no-op `Err` path is the only cost. There
    /// is deliberately no `HashSet` dedup here: it would allocate a transient
    /// structure that contaminates `bench_memory`'s `read_allocated` delta and
    /// penalizes `bench_insert` with per-key hashing the tree doesn't need.
    fn build_tree(keys: &[Vec<u8>]) -> (Tree, Vec<u8>) {
        let mut tree = Tree::new();
        let mut max_key: Vec<u8> = Vec::new();
        for (i, k) in keys.iter().enumerate() {
            if tree.insert(Box::from(k.as_slice()), i).is_ok() && k > &max_key {
                max_key = k.clone();
            }
        }
        (tree, max_key)
    }
}

impl Benchable for CTreeBench {
    fn key_domain(&self) -> KeyDomain {
        KeyDomain::Any
    }

    fn build(&mut self, keys: &[Vec<u8>], _ctx: &BenchContext) {
        let (mut tree, max_key) = Self::build_tree(keys);
        tree.compact();
        self.tree = tree;
        self.max_key = max_key;
    }

    fn bench_insert(&self, keys: &[Vec<u8>]) -> Option<()> {
        let (tree, _) = Self::build_tree(keys);
        black_box(&tree);
        Some(())
    }

    fn bench_lookup(&self, ctx: &BenchContext) -> Option<()> {
        for k in &ctx.lookup_keys {
            black_box(self.tree.get(k.as_slice()));
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
        // Position at the largest key, then walk backward.
        let mut it = self.tree.cursor_at(self.max_key.as_slice());
        while let Some((k, v)) = it.current() {
            black_box(k);
            black_box(v);
            if it.prev().is_none() {
                break;
            }
        }
        Some(())
    }

    fn bench_memory(&self, keys: &[Vec<u8>]) -> Option<f64> {
        let before = read_allocated();
        let (mut tree, _) = Self::build_tree(keys);
        // Measure the tree's steady-state footprint, not its transient
        // growth capacity: `leaves`/`inodes` are `Vec`s that double during
        // inserts, leaving spare capacity that's real resident bytes but not
        // part of the tree's logical size. Compacting reclaims it.
        tree.compact();
        let bytes = read_allocated() - before;
        drop(tree);
        Some(bytes as f64 / keys.len() as f64)
    }
}

// ── CTreeOpt ─────────────────────────────────────────────────────────

/// `CTreeBench` variant that calls `optimize` after building.
///
/// `optimize` permutes the leaf arena into linked-list order so forward
/// iteration becomes a contiguous sweep. This contestant isolates the
/// iteration/lookup payoff from the one-time reorder cost: `build` pays it
/// once, then `bench_fwd_iter`/`bench_rev_iter`/`bench_lookup` measure the
/// optimized layout. `bench_optimize` measures the reorder cost itself.
pub(crate) struct CTreeOptBench {
    tree: Tree,
    max_key: Vec<u8>,
}

impl CTreeOptBench {
    pub(crate) fn new() -> Self {
        Self { tree: Tree::new(), max_key: Vec::new() }
    }
}

impl Benchable for CTreeOptBench {
    fn key_domain(&self) -> KeyDomain {
        KeyDomain::Any
    }

    fn build(&mut self, keys: &[Vec<u8>], _ctx: &BenchContext) {
        let (mut tree, max_key) = CTreeBench::build_tree(keys);
        tree.compact();
        tree.optimize();
        self.tree = tree;
        self.max_key = max_key;
    }

    fn bench_insert(&self, keys: &[Vec<u8>]) -> Option<()> {
        let (tree, _) = CTreeBench::build_tree(keys);
        black_box(&tree);
        Some(())
    }

    fn bench_optimize(&self, keys: &[Vec<u8>]) -> Option<()> {
        let (mut tree, _) = CTreeBench::build_tree(keys);
        tree.compact();
        tree.optimize();
        black_box(&tree);
        Some(())
    }

    fn bench_lookup(&self, ctx: &BenchContext) -> Option<()> {
        for k in &ctx.lookup_keys {
            black_box(self.tree.get(k.as_slice()));
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
        let mut it = self.tree.cursor_at(self.max_key.as_slice());
        while let Some((k, v)) = it.current() {
            black_box(k);
            black_box(v);
            if it.prev().is_none() {
                break;
            }
        }
        Some(())
    }

    fn bench_memory(&self, keys: &[Vec<u8>]) -> Option<f64> {
        let before = read_allocated();
        let (mut tree, _) = CTreeBench::build_tree(keys);
        tree.compact();
        tree.optimize();
        let bytes = read_allocated() - before;
        drop(tree);
        Some(bytes as f64 / keys.len() as f64)
    }
}
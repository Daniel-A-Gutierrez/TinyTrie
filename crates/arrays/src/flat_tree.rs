//! Leaf-node comparison for **interned variable-length keys**: SortedArray vs
//! FlatTree (fnode) vs DecisionTree.
//!
//! The tree is still a single leaf node — keys are *interned* in a side
//! `Vec<Box<[u8]>>` and the node stores only `(depth, symbol)` navigation plus a
//! `ptr` into that table. The scan runs over the node's slots/nodes and ends with a
//! single full-key check (`keys[ptr-1] == query`).
//!
//! - **SortedArray** — `Vec<(Box<[u8]>, u8)>` sorted; binary-search baseline.
//! - **FlatTree (fnode)** — `TinyArray<(depth, symbol, ptr), CAP>` of pre-order
//!   slots. Lookup is the depth-tracking linear scan from the notes: at the current
//!   `depth`, skip slots with `d > depth` (deeper children), break on `d < depth`
//!   (left the subtree), match `query[depth] == symbol`, then advance `depth` to the
//!   next slot's depth (the child level). The last matched `ptr` is the candidate.
//! - **DecisionTree** — `TinyArray<(depth, symbol, next, ptr), CAP>` of pre-order
//!   nodes. Same trie, but `next` is the index of the next **sibling** (it jumps over
//!   the node's deeper children), so the scan descends to the child (`i+1`, verified
//!   by depth) on a match and follows `next` on a mismatch — no linear scan past
//!   `d > depth` slots. `0xFF` ends a sibling chain.

#![allow(dead_code)]

use std::mem::size_of;
use std::time::Instant;

use crate::tiny_array::TinyArray;

type Key = Box<[u8]>;

// ============================================================================
// SortedArray — Vec<(Box<[u8]>, u8)>, sorted
// ============================================================================

pub struct SortedArray {
    entries: Vec<(Key, u8)>,
}

impl SortedArray {
    pub fn new() -> Self {
        Self { entries: Vec::new() }
    }

    pub fn insert(&mut self, k: &[u8], v: u8) {
        match self.entries.binary_search_by(|(e, _)| e.as_ref().cmp(k)) {
            Ok(_) => {} // duplicate
            Err(pos) => self.entries.insert(pos, (Box::from(k), v)),
        }
    }

    pub fn get(&self, k: &[u8]) -> Option<u8> {
        self.entries
            .binary_search_by(|(e, _)| e.as_ref().cmp(k))
            .ok()
            .map(|i| self.entries[i].1)
    }

    pub fn iter_count(&self) -> usize {
        let mut sum: u64 = 0;
        for (_, v) in self.entries.iter() {
            sum = sum.wrapping_add(*v as u64);
        }
        std::hint::black_box(sum);
        self.entries.len()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

// ============================================================================
// FlatTree (fnode) — pre-order (depth, symbol, ptr) slots, depth-tracking scan
// ============================================================================

const FT_CAP: usize = 256;

#[derive(Clone, Copy)]
struct FSlot {
    depth: u8,
    symbol: u8,
    ptr: u8, // 0 = no terminal here; else 1-based index into the interned key table
}

pub struct FlatTree {
    slots: TinyArray<FSlot, FT_CAP>,
    keys: Vec<Key>,   // interned key table; keys[ptr-1] is the key for slot `ptr`
    values: Vec<u8>,  // parallel to keys
}

impl FlatTree {
    /// Batch-build the pre-order micro-trie from lexicographically-sorted keys.
    pub fn build(keys: &[Key], values: &[u8]) -> Self {
        let mut slots = TinyArray::new();
        build_flat(0, 0, keys.len(), keys, &mut slots);
        Self { slots, keys: keys.to_vec(), values: values.to_vec() }
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// Depth-tracking linear scan (the fnode scan from the notes). Walks slots in
    /// pre-order, tracking `depth`; the deepest matched `ptr` is the candidate,
    /// verified by a final full-key comparison.
    pub fn get(&self, query: &[u8]) -> Option<u8> {
        let n = self.slots.len();
        if n == 0 {
            return None;
        }
        let l = query.len();
        let mut depth = self.slots.get(0).depth as usize;
        if depth >= l {
            return None;
        }
        let mut child: u8 = 0;
        for i in 0..n {
            let s = *self.slots.get(i);
            let d = s.depth as usize;
            if d < depth {
                break;
            }
            if d > depth {
                continue;
            }
            if query[depth] == s.symbol {
                child = s.ptr;
                if i + 1 == n {
                    break;
                }
                let next_d = self.slots.get(i + 1).depth as usize;
                if next_d <= depth || next_d >= l {
                    break;
                }
                depth = next_d;
            }
        }
        if child != 0 {
            let idx = (child - 1) as usize;
            if self.keys[idx].as_ref() == query {
                return Some(self.values[idx]);
            }
        }
        None
    }

    /// Pre-order slot order == sorted key order; emit terminals (ptr != 0).
    pub fn iter_count(&self) -> usize {
        let n = self.slots.len();
        let mut sum: u64 = 0;
        let mut count = 0;
        for i in 0..n {
            let ptr = self.slots.get(i).ptr;
            if ptr != 0 {
                sum = sum.wrapping_add(self.values[(ptr - 1) as usize] as u64);
                count += 1;
            }
        }
        std::hint::black_box(sum);
        count
    }
}

/// Recursive pre-order builder. `keys[kstart..kend]` all share `bytes[..depth]` and
/// each has length `> depth`. Group by `keys[i][depth]`; the shortest key in a group
/// (length `depth+1`, which sorts first) is the terminal for that slot; the rest are
/// longer and recurse as children at `depth+1`.
fn build_flat(depth: usize, kstart: usize, kend: usize, keys: &[Key], slots: &mut TinyArray<FSlot, FT_CAP>) {
    let mut i = kstart;
    while i < kend {
        let s = keys[i][depth];
        let mut j = i + 1;
        while j < kend && keys[j][depth] == s {
            j += 1;
        }
        let mut ptr = 0u8;
        let mut child_start = i;
        if keys[i].len() == depth + 1 {
            ptr = (i + 1) as u8; // 1-based key index
            child_start = i + 1; // remaining keys are longer → children
        }
        slots.push(FSlot { depth: depth as u8, symbol: s, ptr });
        if child_start < j {
            build_flat(depth + 1, child_start, j, keys, slots);
        }
        i = j;
    }
}

// ============================================================================
// DecisionTree — pre-order (depth, symbol, next, ptr); next skips the subtree
// ============================================================================

const DT_CAP: usize = 256;
const END: u8 = 0xFF; // end-of-chain sentinel (index 255 reserved)

#[derive(Clone, Copy)]
struct DNode {
    depth: u8,
    symbol: u8,
    next: u8, // index of the next sibling (skips this node's child subtree), or END
    ptr: u8,  // 0 = no terminal here; else 1-based index into the interned key table
}

pub struct DecisionTree {
    nodes: TinyArray<DNode, DT_CAP>,
    keys: Vec<Key>,
    values: Vec<u8>,
}

impl DecisionTree {
    pub fn build(keys: &[Key], values: &[u8]) -> Self {
        let mut nodes = TinyArray::new();
        build_dt(0, 0, keys.len(), keys, &mut nodes);
        Self { nodes, keys: keys.to_vec(), values: values.to_vec() }
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// Scan: match → descend to the child at `i+1` (verified deeper); mismatch →
    /// follow `next` to the next sibling (skipping the child subtree). On exhausting
    /// the query at a terminal, do a final full-key check.
    pub fn get(&self, query: &[u8]) -> Option<u8> {
        let n = self.nodes.len();
        if n == 0 {
            return None;
        }
        let l = query.len();
        let mut i = 0u8;
        loop {
            if i == END {
                return None;
            }
            let node = *self.nodes.get(i as usize);
            let d = node.depth as usize;
            if d >= l {
                return None; // node compares a byte beyond the query → query is a prefix
            }
            if node.symbol == query[d] {
                if d + 1 == l {
                    // query exhausted at this match
                    if node.ptr != 0 {
                        let idx = (node.ptr - 1) as usize;
                        if self.keys[idx].as_ref() == query {
                            return Some(self.values[idx]);
                        }
                    }
                    return None;
                }
                // descend to the child (pre-order: immediately after this node)
                let ni = i as usize + 1;
                if ni < n && (self.nodes.get(ni).depth as usize) > d {
                    i = ni as u8;
                } else {
                    return None; // no child, query not exhausted → miss
                }
            } else {
                i = node.next; // next sibling, skipping the child subtree
            }
        }
    }

    /// Pre-order node order == sorted key order; emit terminals (ptr != 0).
    pub fn iter_count(&self) -> usize {
        let n = self.nodes.len();
        let mut sum: u64 = 0;
        let mut count = 0;
        for i in 0..n {
            let ptr = self.nodes.get(i).ptr;
            if ptr != 0 {
                sum = sum.wrapping_add(self.values[(ptr - 1) as usize] as u64);
                count += 1;
            }
        }
        std::hint::black_box(sum);
        count
    }
}

/// Recursive pre-order builder. Emits node, then its child subtree, then siblings.
/// `next` of a node = the index of the next sibling (= index after its subtree) if a
/// sibling exists, else `END`. Returns the node index right after this group's
/// subtree (used by the parent to set its child's `next`).
fn build_dt(
    depth: usize,
    kstart: usize,
    kend: usize,
    keys: &[Key],
    nodes: &mut TinyArray<DNode, DT_CAP>,
) -> u32 {
    let mut i = kstart;
    while i < kend {
        let s = keys[i][depth];
        let mut j = i + 1;
        while j < kend && keys[j][depth] == s {
            j += 1;
        }
        let node_idx = nodes.len() as u32;
        let mut ptr = 0u8;
        let mut child_start = i;
        if keys[i].len() == depth + 1 {
            ptr = (i + 1) as u8;
            child_start = i + 1;
        }
        nodes.push(DNode { depth: depth as u8, symbol: s, next: END, ptr });
        let after_children = if child_start < j {
            build_dt(depth + 1, child_start, j, keys, nodes)
        } else {
            node_idx + 1
        };
        nodes.get_mut(node_idx as usize).next = if j < kend { after_children as u8 } else { END };
        i = j;
    }
    nodes.len() as u32
}

// ============================================================================
// Benchmark harness
// ============================================================================

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 32
    }
}

/// Generate `n` distinct interned keys (length 2 or 3, bytes 0..16 for sharing),
/// returned lexicographically sorted. Deterministic.
fn gen_keys(n: usize) -> Vec<Key> {
    let mut rng = Lcg(0x9E3779B97F4A7C15);
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<Key> = Vec::with_capacity(n);
    while out.len() < n {
        let len = 2 + (rng.next() as usize % 2); // 2 or 3
        let mut k = vec![0u8; len];
        for b in k.iter_mut() {
            *b = (rng.next() & 0x0F) as u8; // 0..16
        }
        let kb: Key = k.into_boxed_slice();
        if seen.insert(kb.clone()) {
            out.push(kb);
        }
    }
    out.sort_by(|a, b| a.as_ref().cmp(b.as_ref()));
    out
}

fn gen_miss(keys: &[Key]) -> Vec<Key> {
    keys.iter()
        .map(|k| {
            let mut m = k.to_vec();
            let last = m.len() - 1;
            m[last] = (m[last].wrapping_add(1)) & 0x0F; // flip last byte within alphabet
            m.into_boxed_slice()
        })
        .collect()
}

fn perm(n: usize) -> Vec<usize> {
    let mut rng = Lcg(0xD1B54A32D192ED03);
    let mut p: Vec<usize> = (0..n).collect();
    for i in (1..n).rev() {
        let j = (rng.next() as usize) % (i + 1);
        p.swap(i, j);
    }
    p
}

struct Timing {
    build_ns: f64,
    lookup_ns: f64,
    iter_ns: f64,
}

fn time<F: FnMut()>(iters: u64, mut f: F) -> f64 {
    for _ in 0..iters.min(2048) {
        f();
    }
    let start = Instant::now();
    for _ in 0..iters {
        f();
    }
    start.elapsed().as_nanos() as f64 / iters as f64
}

fn bench_sorted(keys: &[Key], values: &[u8], order: &[usize]) -> Timing {
    let n = keys.len();
    let build_iters = 200_000 / (n.max(1) as u64);
    let lookup_iters = 2_000_000;
    let iter_iters = 1_000_000;

    // Incremental insert in random order (realistic per-key insert cost).
    let build_ns = time(build_iters, || {
        let mut a = SortedArray::new();
        for &i in order {
            a.insert(&keys[i], values[i]);
        }
        std::hint::black_box(&a);
    }) / n as f64;

    let mut a = SortedArray::new();
    for &i in order {
        a.insert(&keys[i], values[i]);
    }

    let miss = gen_miss(keys);
    let lookup_ns = time(lookup_iters, || {
        let mut acc = 0u8;
        for i in 0..n {
            acc |= a.get(keys[i].as_ref()).unwrap_or(0);
            acc |= a.get(miss[i].as_ref()).unwrap_or(0);
        }
        std::hint::black_box(acc);
    }) / (2 * n) as f64;

    let iter_ns = time(iter_iters, || {
        a.iter_count();
    }) / n.max(1) as f64;

    Timing { build_ns, lookup_ns, iter_ns }
}

fn bench_flat(keys: &[Key], values: &[u8]) -> Timing {
    let n = keys.len();
    let build_iters = 200_000 / (n.max(1) as u64);
    let lookup_iters = 2_000_000;
    let iter_iters = 1_000_000;

    // Time only the slot construction (keys are interned once, outside the loop).
    let build_ns = time(build_iters, || {
        let mut slots = TinyArray::new();
        build_flat(0, 0, n, keys, &mut slots);
        std::hint::black_box(&slots);
    }) / n as f64;

    let t = FlatTree::build(keys, values);
    let miss = gen_miss(keys);
    let lookup_ns = time(lookup_iters, || {
        let mut acc = 0u8;
        for i in 0..n {
            acc |= t.get(keys[i].as_ref()).unwrap_or(0);
            acc |= t.get(miss[i].as_ref()).unwrap_or(0);
        }
        std::hint::black_box(acc);
    }) / (2 * n) as f64;

    let iter_ns = time(iter_iters, || {
        t.iter_count();
    }) / n.max(1) as f64;

    Timing { build_ns, lookup_ns, iter_ns }
}

fn bench_decision(keys: &[Key], values: &[u8]) -> Timing {
    let n = keys.len();
    let build_iters = 200_000 / (n.max(1) as u64);
    let lookup_iters = 2_000_000;
    let iter_iters = 1_000_000;

    let build_ns = time(build_iters, || {
        let mut nodes = TinyArray::new();
        build_dt(0, 0, n, keys, &mut nodes);
        std::hint::black_box(&nodes);
    }) / n as f64;

    let t = DecisionTree::build(keys, values);
    let miss = gen_miss(keys);
    let lookup_ns = time(lookup_iters, || {
        let mut acc = 0u8;
        for i in 0..n {
            acc |= t.get(keys[i].as_ref()).unwrap_or(0);
            acc |= t.get(miss[i].as_ref()).unwrap_or(0);
        }
        std::hint::black_box(acc);
    }) / (2 * n) as f64;

    let iter_ns = time(iter_iters, || {
        t.iter_count();
    }) / n.max(1) as f64;

    Timing { build_ns, lookup_ns, iter_ns }
}

/// Per-node and per-structure memory footprint.
fn print_sizes() {
    println!("== memory (size_of, 64-bit) ==");
    println!(
        "  FSlot (depth,symbol,ptr)        = {} bytes   | TinyArray<FSlot,256> arena = {} bytes",
        size_of::<FSlot>(),
        size_of::<TinyArray<FSlot, FT_CAP>>(),
    );
    println!(
        "  DNode (depth,symbol,next,ptr)   = {} bytes   | TinyArray<DNode,256> arena = {} bytes",
        size_of::<DNode>(),
        size_of::<TinyArray<DNode, DT_CAP>>(),
    );
    println!(
        "  SortedArray entry (Box<[u8]>,u8) = {} bytes (fat ptr 16 + value 1, padded)",
        size_of::<(Key, u8)>(),
    );
    println!(
        "  Box<[u8]> (interned key handle)  = {} bytes  | Vec<u8> header = {} bytes",
        size_of::<Key>(),
        size_of::<Vec<u8>>(),
    );
    println!();
}

pub fn run_benchmarks() {
    println!("Leaf-node comparison (interned Box<[u8]> keys): SortedArray vs FlatTree vs DecisionTree");
    println!("Keys len 2-3, bytes 0..16. Times in ns (lower better).\n");

    print_sizes();

    let sizes: &[(&str, usize)] = &[("16 keys", 16), ("32 keys", 32), ("64 keys", 64)];

    for &(label, n) in sizes {
        let keys = gen_keys(n);
        let values: Vec<u8> = (0..n as u8).collect();
        let order = perm(n);
        let s = bench_sorted(&keys, &values, &order);
        let f = bench_flat(&keys, &values);
        let d = bench_decision(&keys, &values);

        println!("─[ {} ]─────────────────────────────────────", label);
        println!("{:<14} {:>14} {:>14} {:>14}", "structure", "build/key", "lookup", "iter/key");
        println!("{:<14} {:>14.2} {:>14.2} {:>14.2}", "SortedArray", s.build_ns, s.lookup_ns, s.iter_ns);
        println!("{:<14} {:>14.2} {:>14.2} {:>14.2}", "FlatTree", f.build_ns, f.lookup_ns, f.iter_ns);
        println!("{:<14} {:>14.2} {:>14.2} {:>14.2}", "DecisionTree", d.build_ns, d.lookup_ns, d.iter_ns);
        println!();
    }

    correctness_check();
}

fn correctness_check() {
    // Mixed lengths with a key that is a prefix of another (terminal + children).
    let keys_raw: Vec<&[u8]> = vec![b"ab", b"abc", b"ac", b"ba", b"b", b"abd"];
    let keys: Vec<Key> = keys_raw.iter().map(|k| Box::from(*k)).collect();
    let mut sorted = keys.clone();
    sorted.sort_by(|a, b| a.as_ref().cmp(b.as_ref()));
    let values: Vec<u8> = (0..sorted.len() as u8).collect();

    let sa_keys = sorted.clone();
    let mut sa = SortedArray::new();
    for (i, k) in sa_keys.iter().enumerate() {
        sa.insert(k, values[i]);
    }
    let ft = FlatTree::build(&sorted, &values);
    let dt = DecisionTree::build(&sorted, &values);

    for (i, k) in sorted.iter().enumerate() {
        assert_eq!(sa.get(k), Some(values[i]), "SortedArray miss {:?}", k);
        assert_eq!(ft.get(k), Some(values[i]), "FlatTree miss {:?}", k);
        assert_eq!(dt.get(k), Some(values[i]), "DecisionTree miss {:?}", k);
    }
    // misses: prefixes that aren't keys, and divergent suffixes
    let misses: Vec<&[u8]> = vec![b"a", b"abx", b"bb", b"bc", b"abcd", b""];
    for m in misses {
        assert_eq!(sa.get(m), None, "SortedArray false hit {:?}", m);
        assert_eq!(ft.get(m), None, "FlatTree false hit {:?}", m);
        assert_eq!(dt.get(m), None, "DecisionTree false hit {:?}", m);
    }
    assert_eq!(sa.iter_count(), sorted.len());
    assert_eq!(ft.iter_count(), sorted.len());
    assert_eq!(dt.iter_count(), sorted.len());

    // randomized cross-check vs SortedArray over many key sets
    let mut rng = Lcg(0xABCDEF1234567890);
    for _ in 0..200 {
        let m = 1 + (rng.next() as usize % 40);
        let keys = gen_keys(m);
        let values: Vec<u8> = (0..m as u8).collect();
        let ft = FlatTree::build(&keys, &values);
        let dt = DecisionTree::build(&keys, &values);
        let mut sa = SortedArray::new();
        for (i, k) in keys.iter().enumerate() {
            sa.insert(k, values[i]);
        }
        for (i, k) in keys.iter().enumerate() {
            assert_eq!(ft.get(k), Some(values[i]), "FlatTree randomized miss key {:?}", k);
            assert_eq!(dt.get(k), Some(values[i]), "DecisionTree randomized miss key {:?}", k);
            assert_eq!(sa.get(k), ft.get(k));
        }
        let miss = gen_miss(&keys);
        for k in miss.iter() {
            assert_eq!(ft.get(k), sa.get(k), "FlatTree miss-disagreement {:?}", k);
            assert_eq!(dt.get(k), sa.get(k), "DecisionTree miss-disagreement {:?}", k);
        }
        assert_eq!(ft.iter_count(), sa.iter_count());
        assert_eq!(dt.iter_count(), sa.iter_count());
    }

    println!("correctness: OK (hand-crafted prefix/children case + 200 randomized cross-checks vs SortedArray)");
}
use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::{BTreeMap, HashMap, LinkedList};
use std::hint::black_box;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tiny_trie::{BitTrie, DynNibbleTrie, NibbleTrie, PolyTrie, TinyTrie, TinyTrieMap};

// NibbleTrie with u32 LEN so buf can hold >64KB (needed for 10K+ keys).
type NT = NibbleTrie<usize, u32, u32>;

// ── Config ──────────────────────────────────────────────────────────

const SIZES: &[usize] = &[10, 100, 1000, 100_000, 10_000_000];
const BENCH_SECS: u64 = 2;
const COL: usize = 16; // table column width

// ── Allocation tracker ──────────────────────────────────────────────

#[global_allocator]
static TRACKER: TrackingAllocator = TrackingAllocator;

struct TrackingAllocator;
static ALLOCATED: AtomicU64 = AtomicU64::new(0);

unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATED.fetch_add(layout.size() as u64, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        ALLOCATED.fetch_sub(layout.size() as u64, Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }
}

fn read_allocated() -> u64 {
    ALLOCATED.load(Ordering::Relaxed)
}

// ── Key generation (guaranteed unique) ─────────────────────────────

fn string_keys(n: usize) -> Vec<Vec<u8>> {
    let w = format!("{}", n - 1).len();
    (0..n).map(|i| format!("key_{i:0>w$}").into_bytes()).collect()
}

// ── Sorted-vec helpers ──────────────────────────────────────────────

fn build_sorted_vec(keys: &[Vec<u8>]) -> Vec<(Vec<u8>, usize)> {
    let mut v: Vec<_> = keys.iter().enumerate().map(|(i, k)| (k.clone(), i)).collect();
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v
}

fn sorted_vec_get(sv: &[(Vec<u8>, usize)], key: &[u8]) -> Option<usize> {
    sv.binary_search_by(|e| e.0.as_slice().cmp(key)).ok().map(|i| sv[i].1)
}

// ── Pre-built structures for lookup / iteration ───────────────────────

struct Structures {
    trie: TinyTrie<usize, 6, u8>,
    ntrie: NT,
    ntrie_opt: NT,
    btrie: BitTrie<usize>,
    ptrie: PolyTrie<usize>,
    ptrie_opt: PolyTrie<usize>,
    dyn_ntrie: DynNibbleTrie<usize>,
    dyn_ntrie_opt: DynNibbleTrie<usize>,
    btree: BTreeMap<Vec<u8>, usize>,
    hmap: HashMap<Vec<u8>, usize>,
    sorted: Vec<(Vec<u8>, usize)>,
    llist: LinkedList<(Vec<u8>, usize)>,
    lookup_keys: Vec<Vec<u8>>,
    lookup_keys_null: Vec<Vec<u8>>,
    /// Keys that are known to be in the trie (for unchecked lookup benches).
    hit_keys: Vec<Vec<u8>>,
}

fn build_all(keys: &[Vec<u8>]) -> Structures {
    let mut trie = TinyTrie::new();
    let mut ntrie = NT::new();
    let mut btrie = BitTrie::new();
    let mut ptrie = PolyTrie::new();
    let mut dyn_ntrie = DynNibbleTrie::new();
    let mut btree = BTreeMap::new();
    let mut hmap = HashMap::new();
    for (i, k) in keys.iter().enumerate() {
        trie.insert(k.clone(), i).unwrap();
        ntrie.insert(k.clone(), i).unwrap();
        btrie.insert(k.clone(), i).unwrap();
        ptrie.insert(k.clone(), i).unwrap();
        dyn_ntrie.insert(k.clone(), i).unwrap();
        btree.insert(k.clone(), i);
        hmap.insert(k.clone(), i);
    }
    let llist: LinkedList<_> = keys.iter().enumerate().map(|(i, k)| (k.clone(), i)).collect();
    let mut lookup_keys = Vec::with_capacity(keys.len() * 2);
    let mut lookup_keys_null = Vec::with_capacity(keys.len() * 2);
    for k in keys {
        lookup_keys.push(k.clone());
        let mut nt = k.clone();
        nt.push(0);
        lookup_keys_null.push(nt);
        let mut miss = k.clone();
        miss.push(b'z');
        lookup_keys.push(miss.clone());
        miss.push(0);
        lookup_keys_null.push(miss);
    }
    let mut ptrie_opt = ptrie.clone();
    ptrie_opt.optimize();
    let mut ntrie_opt = ntrie.clone();
    ntrie_opt.optimize();
    let mut dyn_ntrie_opt = DynNibbleTrie::new();
    for (i, k) in keys.iter().enumerate() { dyn_ntrie_opt.insert(k.clone(), i).unwrap(); }
    dyn_ntrie_opt.optimize();
    Structures { trie, ntrie, ntrie_opt, btrie, ptrie, ptrie_opt, dyn_ntrie, dyn_ntrie_opt, btree, hmap, sorted: build_sorted_vec(keys), llist, lookup_keys, lookup_keys_null, hit_keys: keys.to_vec() }
}

// ── Bench harness ───────────────────────────────────────────────────

struct BenchResult {
    iters: u64,
    elapsed: Duration,
}

impl BenchResult {
    fn rate(&self, ops_per_iter: u64) -> f64 {
        (self.iters * ops_per_iter) as f64 / self.elapsed.as_secs_f64()
    }
}

/// Run `f` repeatedly until `budget` has elapsed, counting iterations.
fn bench(budget: Duration, label: &str, f: impl Fn()) -> BenchResult {
    let mut iters = 0u64;
    let start = Instant::now();
    loop {
        f();
        iters += 1;
        if start.elapsed() >= budget {
            break;
        }
    }
    let elapsed = start.elapsed();
    let per = elapsed.as_secs_f64() / iters as f64;
    if per >= 1.0 {
        eprintln!("    {label}: {iters} iters in {:.2}s ({:.2}s/iter) ✓", elapsed.as_secs_f64(), per);
    } else if per >= 0.001 {
        eprintln!("    {label}: {iters} iters in {:.2}s ({:.1}ms/iter) ✓", elapsed.as_secs_f64(), per * 1000.0);
    } else {
        eprintln!("    {label}: {iters} iters in {:.2}s ({:.1}µs/iter) ✓", elapsed.as_secs_f64(), per * 1e6);
    }
    BenchResult { iters, elapsed }
}

// ── Contestant definitions ──────────────────────────────────────────
//
// Each contestant is a unit struct that implements whichever bench traits
// are applicable. The dispatch in main() uses trait objects.
//
// Ops is a bitmask tracking which benchmarks this contestant supports.

#[derive(Clone, Copy)]
struct Ops(u32);

impl Ops {
    const INSERT:   Ops = Ops(1 << 0);
    const LOOKUP:   Ops = Ops(1 << 1);
    const FWD_ITER: Ops = Ops(1 << 2);
    const REV_ITER: Ops = Ops(1 << 3);
    const FWD_IDX:  Ops = Ops(1 << 4);
    const REV_IDX:  Ops = Ops(1 << 5);
    const OPTIMIZE: Ops = Ops(1 << 6);
    const MEMORY:   Ops = Ops(1 << 7);

    fn contains(self, flag: Ops) -> bool { self.0 & flag.0 != 0 }
}

impl std::ops::BitOr for Ops {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self { Ops(self.0 | rhs.0) }
}

// ── Bench traits ────────────────────────────────────────────────────

trait InsertBench {
    fn run(&self, keys: &[Vec<u8>]);
}

trait LookupBench {
    fn run(&self, st: &Structures);
}

trait FwdIterBench {
    fn run(&self, st: &Structures);
}

trait RevIterBench {
    fn run(&self, st: &Structures);
}

trait FwdIdxBench {
    fn run(&self, st: &Structures);
}

trait RevIdxBench {
    fn run(&self, st: &Structures);
}

trait OptimizeBench {
    fn run(&self, keys: &[Vec<u8>]);
}

trait MemBench {
    fn run(&self, keys: &[Vec<u8>]) -> u64;
}

// ── Contestant unit structs ─────────────────────────────────────────

struct TinyTrieBench;
struct NibbleTrieBench;
struct BitTrieBench;
struct PolyTrieBench;
struct BTreeMapBench;
struct HashMapBench;
struct SortedVecBench;
struct NibbleOptBench;
struct PolyOptBench;
struct LinkedListBench;
struct NibbleUncheckedBench;
struct NibbleOptUncheckedBench;
struct DynNibbleTrieBench;
struct DynNibbleOptBench;

// ── Contestant registry ────────────────────────────────────────────

struct Contestant {
    name: &'static str,
    ops: Ops,
    /// Skip this contestant for sizes larger than this (None = no limit).
    /// Prevents O(n²) algorithms from hanging the suite at large N.
    max_size: Option<usize>,
    insert: Option<Box<dyn InsertBench>>,
    lookup: Option<Box<dyn LookupBench>>,
    fwd_iter: Option<Box<dyn FwdIterBench>>,
    rev_iter: Option<Box<dyn RevIterBench>>,
    fwd_idx: Option<Box<dyn FwdIdxBench>>,
    rev_idx: Option<Box<dyn RevIdxBench>>,
    optimize: Option<Box<dyn OptimizeBench>>,
    mem: Option<Box<dyn MemBench>>,
}

fn all_contestants() -> Vec<Contestant> {
    vec![
        Contestant {
            name: "TinyTrie", max_size: None,
            ops: Ops::INSERT | Ops::LOOKUP | Ops::FWD_ITER | Ops::REV_ITER | Ops::MEMORY,
            insert: Some(Box::new(TinyTrieBench)),
            lookup: Some(Box::new(TinyTrieBench)),
            fwd_iter: Some(Box::new(TinyTrieBench)),
            rev_iter: Some(Box::new(TinyTrieBench)),
            fwd_idx: None, rev_idx: None, optimize: None,
            mem: Some(Box::new(TinyTrieBench)),
        },
        Contestant {
            name: "NibbleTrie", max_size: None,
            ops: Ops::INSERT | Ops::LOOKUP | Ops::FWD_ITER | Ops::REV_ITER | Ops::FWD_IDX | Ops::REV_IDX | Ops::MEMORY,
            insert: Some(Box::new(NibbleTrieBench)),
            lookup: Some(Box::new(NibbleTrieBench)),
            fwd_iter: Some(Box::new(NibbleTrieBench)),
            rev_iter: Some(Box::new(NibbleTrieBench)),
            fwd_idx: Some(Box::new(NibbleTrieBench)),
            rev_idx: Some(Box::new(NibbleTrieBench)), optimize: None,
            mem: Some(Box::new(NibbleTrieBench)),
        },
        Contestant {
            name: "BitTrie", max_size: None,
            ops: Ops::INSERT | Ops::LOOKUP | Ops::FWD_ITER | Ops::REV_ITER | Ops::MEMORY,
            insert: Some(Box::new(BitTrieBench)),
            lookup: Some(Box::new(BitTrieBench)),
            fwd_iter: Some(Box::new(BitTrieBench)),
            rev_iter: Some(Box::new(BitTrieBench)),
            fwd_idx: None, rev_idx: None, optimize: None,
            mem: Some(Box::new(BitTrieBench)),
        },
        Contestant {
            name: "PolyTrie", max_size: None,
            ops: Ops::INSERT | Ops::LOOKUP | Ops::FWD_ITER | Ops::REV_ITER | Ops::MEMORY,
            insert: Some(Box::new(PolyTrieBench)),
            lookup: Some(Box::new(PolyTrieBench)),
            fwd_iter: Some(Box::new(PolyTrieBench)),
            rev_iter: Some(Box::new(PolyTrieBench)),
            fwd_idx: None, rev_idx: None, optimize: None,
            mem: Some(Box::new(PolyTrieBench)),
        },
        Contestant {
            name: "BTreeMap", max_size: None,
            ops: Ops::INSERT | Ops::LOOKUP | Ops::FWD_ITER | Ops::REV_ITER | Ops::MEMORY,
            insert: Some(Box::new(BTreeMapBench)),
            lookup: Some(Box::new(BTreeMapBench)),
            fwd_iter: Some(Box::new(BTreeMapBench)),
            rev_iter: Some(Box::new(BTreeMapBench)),
            fwd_idx: None, rev_idx: None, optimize: None,
            mem: Some(Box::new(BTreeMapBench)),
        },
        Contestant {
            name: "HashMap", max_size: None,
            ops: Ops::INSERT | Ops::LOOKUP | Ops::MEMORY,
            insert: Some(Box::new(HashMapBench)),
            lookup: Some(Box::new(HashMapBench)),
            fwd_iter: None, rev_iter: None, fwd_idx: None, rev_idx: None, optimize: None,
            mem: Some(Box::new(HashMapBench)),
        },
        Contestant {
            name: "SortedVec", max_size: None,
            ops: Ops::INSERT | Ops::LOOKUP | Ops::FWD_ITER | Ops::MEMORY,
            insert: Some(Box::new(SortedVecBench)),
            lookup: Some(Box::new(SortedVecBench)),
            fwd_iter: Some(Box::new(SortedVecBench)),
            rev_iter: None, fwd_idx: None, rev_idx: None, optimize: None,
            mem: Some(Box::new(SortedVecBench)),
        },
        Contestant {
            name: "NibbleOpt", max_size: None,
            ops: Ops::LOOKUP | Ops::FWD_ITER | Ops::REV_ITER | Ops::FWD_IDX | Ops::REV_IDX | Ops::OPTIMIZE | Ops::MEMORY,
            insert: None,
            lookup: Some(Box::new(NibbleOptBench)),
            fwd_iter: Some(Box::new(NibbleOptBench)),
            rev_iter: Some(Box::new(NibbleOptBench)),
            fwd_idx: Some(Box::new(NibbleOptBench)),
            rev_idx: Some(Box::new(NibbleOptBench)),
            optimize: Some(Box::new(NibbleOptBench)),
            mem: Some(Box::new(NibbleOptBench)),
        },
        Contestant {
            name: "PolyOpt", max_size: None,
            ops: Ops::LOOKUP | Ops::FWD_ITER | Ops::REV_ITER | Ops::OPTIMIZE | Ops::MEMORY,
            insert: None,
            lookup: Some(Box::new(PolyOptBench)),
            fwd_iter: Some(Box::new(PolyOptBench)),
            rev_iter: Some(Box::new(PolyOptBench)),
            fwd_idx: None, rev_idx: None,
            optimize: Some(Box::new(PolyOptBench)),
            mem: Some(Box::new(PolyOptBench)),
        },
        Contestant {
            name: "LinkedList", max_size: None,
            ops: Ops::INSERT | Ops::FWD_ITER | Ops::REV_ITER | Ops::MEMORY,
            insert: Some(Box::new(LinkedListBench)),
            lookup: None,
            fwd_iter: Some(Box::new(LinkedListBench)),
            rev_iter: Some(Box::new(LinkedListBench)),
            fwd_idx: None, rev_idx: None, optimize: None,
            mem: Some(Box::new(LinkedListBench)),
        },
        Contestant {
            name: "NibbleUnchecked", max_size: None,
            ops: Ops::LOOKUP,
            insert: None, lookup: Some(Box::new(NibbleUncheckedBench)),
            fwd_iter: None, rev_iter: None, fwd_idx: None, rev_idx: None, optimize: None, mem: None,
        },
        Contestant {
            name: "NibbleOptUnchecked", max_size: None,
            ops: Ops::LOOKUP,
            insert: None, lookup: Some(Box::new(NibbleOptUncheckedBench)),
            fwd_iter: None, rev_iter: None, fwd_idx: None, rev_idx: None, optimize: None, mem: None,
        },
        Contestant {
            name: "DynNibbleTrie", max_size: None,
            ops: Ops::INSERT | Ops::LOOKUP | Ops::FWD_ITER | Ops::REV_ITER | Ops::MEMORY,
            insert: Some(Box::new(DynNibbleTrieBench)),
            lookup: Some(Box::new(DynNibbleTrieBench)),
            fwd_iter: Some(Box::new(DynNibbleTrieBench)),
            rev_iter: Some(Box::new(DynNibbleTrieBench)),
            fwd_idx: None, rev_idx: None, optimize: None,
            mem: Some(Box::new(DynNibbleTrieBench)),
        },
        Contestant {
            name: "DynNibbleOpt", max_size: None,
            ops: Ops::LOOKUP | Ops::FWD_ITER | Ops::REV_ITER | Ops::OPTIMIZE | Ops::MEMORY,
            insert: None,
            lookup: Some(Box::new(DynNibbleOptBench)),
            fwd_iter: Some(Box::new(DynNibbleOptBench)),
            rev_iter: Some(Box::new(DynNibbleOptBench)),
            fwd_idx: None, rev_idx: None,
            optimize: Some(Box::new(DynNibbleOptBench)),
            mem: Some(Box::new(DynNibbleOptBench)),
        },
    ]
}

// ── Trait implementations ───────────────────────────────────────────

// TinyTrie — uses null-terminated keys for lookup

impl InsertBench for TinyTrieBench {
    fn run(&self, keys: &[Vec<u8>]) {
        let mut m = TinyTrie::<usize, 6, u8>::trie_new();
        for (i, k) in keys.iter().enumerate() { m.trie_insert(k.clone(), i); }
        black_box(&m);
    }
}
impl LookupBench for TinyTrieBench {
    fn run(&self, st: &Structures) {
        for k in &st.lookup_keys_null { black_box(st.trie.trie_get(k)); }
    }
}
impl FwdIterBench for TinyTrieBench {
    fn run(&self, st: &Structures) { st.trie.trie_iter_fwd(|k, v| { black_box(k); black_box(v); }); }
}
impl RevIterBench for TinyTrieBench {
    fn run(&self, st: &Structures) { st.trie.trie_iter_rev(|k, v| { black_box(k); black_box(v); }); }
}
impl MemBench for TinyTrieBench {
    fn run(&self, keys: &[Vec<u8>]) -> u64 {
        let before = read_allocated();
        let mut m: TinyTrie<usize, 6, u8> = TinyTrie::trie_new();
        for (i, k) in keys.iter().enumerate() { m.trie_insert(k.clone(), i); }
        let bytes = read_allocated() - before;
        drop(m);
        bytes
    }
}

// NibbleTrie — plain keys, no null terminator

impl InsertBench for NibbleTrieBench {
    fn run(&self, keys: &[Vec<u8>]) {
        let mut m = NT::trie_new();
        for (i, k) in keys.iter().enumerate() { m.trie_insert(k.clone(), i); }
        black_box(&m);
    }
}
impl LookupBench for NibbleTrieBench {
    fn run(&self, st: &Structures) {
        for k in &st.lookup_keys { black_box(st.ntrie.trie_get(k)); }
    }
}
impl FwdIterBench for NibbleTrieBench {
    fn run(&self, st: &Structures) { st.ntrie.trie_iter_fwd(|k, v| { black_box(k); black_box(v); }); }
}
impl RevIterBench for NibbleTrieBench {
    fn run(&self, st: &Structures) { st.ntrie.trie_iter_rev(|k, v| { black_box(k); black_box(v); }); }
}
impl FwdIdxBench for NibbleTrieBench {
    fn run(&self, st: &Structures) {
        let mut it = st.ntrie.iter();
        if let Some(i) = it.current_index() { black_box(i); }
        while let Some(i) = it.next_index() { black_box(i); }
    }
}
impl RevIdxBench for NibbleTrieBench {
    fn run(&self, st: &Structures) {
        let mut it = st.ntrie.iter_last();
        if let Some(i) = it.current_index() { black_box(i); }
        while let Some(i) = it.prev_index() { black_box(i); }
    }
}
impl MemBench for NibbleTrieBench {
    fn run(&self, keys: &[Vec<u8>]) -> u64 {
        let before = read_allocated();
        let mut m = NT::trie_new();
        for (i, k) in keys.iter().enumerate() { m.trie_insert(k.clone(), i); }
        let bytes = read_allocated() - before;
        drop(m);
        bytes
    }
}

// BitTrie — uses null-terminated keys for lookup

impl InsertBench for BitTrieBench {
    fn run(&self, keys: &[Vec<u8>]) {
        let mut m = BitTrie::<usize>::trie_new();
        for (i, k) in keys.iter().enumerate() { m.trie_insert(k.clone(), i); }
        black_box(&m);
    }
}
impl LookupBench for BitTrieBench {
    fn run(&self, st: &Structures) {
        for k in &st.lookup_keys_null { black_box(st.btrie.trie_get(k)); }
    }
}
impl FwdIterBench for BitTrieBench {
    fn run(&self, st: &Structures) { st.btrie.trie_iter_fwd(|k, v| { black_box(k); black_box(v); }); }
}
impl RevIterBench for BitTrieBench {
    fn run(&self, st: &Structures) { st.btrie.trie_iter_rev(|k, v| { black_box(k); black_box(v); }); }
}
impl MemBench for BitTrieBench {
    fn run(&self, keys: &[Vec<u8>]) -> u64 {
        let before = read_allocated();
        let mut m: BitTrie<usize> = BitTrie::trie_new();
        for (i, k) in keys.iter().enumerate() { m.trie_insert(k.clone(), i); }
        let bytes = read_allocated() - before;
        drop(m);
        bytes
    }
}

// PolyTrie — uses null-terminated keys for lookup

impl InsertBench for PolyTrieBench {
    fn run(&self, keys: &[Vec<u8>]) {
        let mut m = PolyTrie::<usize>::trie_new();
        for (i, k) in keys.iter().enumerate() { m.trie_insert(k.clone(), i); }
        black_box(&m);
    }
}
impl LookupBench for PolyTrieBench {
    fn run(&self, st: &Structures) {
        for k in &st.lookup_keys_null { black_box(st.ptrie.trie_get(k)); }
    }
}
impl FwdIterBench for PolyTrieBench {
    fn run(&self, st: &Structures) { st.ptrie.trie_iter_fwd(|k, v| { black_box(k); black_box(v); }); }
}
impl RevIterBench for PolyTrieBench {
    fn run(&self, st: &Structures) { st.ptrie.trie_iter_rev(|k, v| { black_box(k); black_box(v); }); }
}
impl MemBench for PolyTrieBench {
    fn run(&self, keys: &[Vec<u8>]) -> u64 {
        let before = read_allocated();
        let mut m: PolyTrie<usize> = PolyTrie::trie_new();
        for (i, k) in keys.iter().enumerate() { m.trie_insert(k.clone(), i); }
        let bytes = read_allocated() - before;
        drop(m);
        bytes
    }
}

// BTreeMap

impl InsertBench for BTreeMapBench {
    fn run(&self, keys: &[Vec<u8>]) {
        let mut m = BTreeMap::new();
        for (i, k) in keys.iter().enumerate() { m.insert(k.clone(), i); }
        black_box(&m);
    }
}
impl LookupBench for BTreeMapBench {
    fn run(&self, st: &Structures) {
        for k in &st.lookup_keys { black_box(st.btree.get(k)); }
    }
}
impl FwdIterBench for BTreeMapBench {
    fn run(&self, st: &Structures) { for (k, v) in st.btree.iter() { black_box(k); black_box(v); } }
}
impl RevIterBench for BTreeMapBench {
    fn run(&self, st: &Structures) { for (k, v) in st.btree.iter().rev() { black_box(k); black_box(v); } }
}
impl MemBench for BTreeMapBench {
    fn run(&self, keys: &[Vec<u8>]) -> u64 {
        let before = read_allocated();
        let mut m: BTreeMap<Vec<u8>, usize> = BTreeMap::new();
        for (i, k) in keys.iter().enumerate() { m.insert(k.clone(), i); }
        let bytes = read_allocated() - before;
        drop(m);
        bytes
    }
}

// HashMap

impl InsertBench for HashMapBench {
    fn run(&self, keys: &[Vec<u8>]) {
        let mut m = HashMap::new();
        for (i, k) in keys.iter().enumerate() { m.insert(k.clone(), i); }
        black_box(&m);
    }
}
impl LookupBench for HashMapBench {
    fn run(&self, st: &Structures) {
        for k in &st.lookup_keys { black_box(st.hmap.get(k)); }
    }
}
impl MemBench for HashMapBench {
    fn run(&self, keys: &[Vec<u8>]) -> u64 {
        let before = read_allocated();
        let mut m: HashMap<Vec<u8>, usize> = HashMap::new();
        for (i, k) in keys.iter().enumerate() { m.insert(k.clone(), i); }
        let bytes = read_allocated() - before;
        drop(m);
        bytes
    }
}

// SortedVec

impl InsertBench for SortedVecBench {
    fn run(&self, keys: &[Vec<u8>]) {
        let mut v: Vec<(Vec<u8>, usize)> = Vec::new();
        for (i, k) in keys.iter().enumerate() {
            match v.binary_search_by(|e| e.0.as_slice().cmp(k)) {
                Ok(_) => {}
                Err(pos) => v.insert(pos, (k.clone(), i)),
            }
        }
        black_box(&v);
    }
}
impl LookupBench for SortedVecBench {
    fn run(&self, st: &Structures) {
        for k in &st.lookup_keys { black_box(sorted_vec_get(&st.sorted, k)); }
    }
}
impl FwdIterBench for SortedVecBench {
    fn run(&self, st: &Structures) { for (k, v) in st.sorted.iter() { black_box(k); black_box(v); } }
}
impl MemBench for SortedVecBench {
    fn run(&self, keys: &[Vec<u8>]) -> u64 {
        let before = read_allocated();
        let s = build_sorted_vec(keys);
        let bytes = read_allocated() - before;
        drop(s);
        bytes
    }
}

// NibbleOpt (optimized NibbleTrie)

impl LookupBench for NibbleOptBench {
    fn run(&self, st: &Structures) {
        for k in &st.lookup_keys { black_box(st.ntrie_opt.trie_get(k)); }
    }
}
impl FwdIterBench for NibbleOptBench {
    fn run(&self, st: &Structures) { st.ntrie_opt.trie_iter_fwd(|k, v| { black_box(k); black_box(v); }); }
}
impl RevIterBench for NibbleOptBench {
    fn run(&self, st: &Structures) { st.ntrie_opt.trie_iter_rev(|k, v| { black_box(k); black_box(v); }); }
}
impl FwdIdxBench for NibbleOptBench {
    fn run(&self, st: &Structures) {
        let mut it = st.ntrie_opt.iter();
        if let Some(i) = it.current_index() { black_box(i); }
        while let Some(i) = it.next_index() { black_box(i); }
    }
}
impl RevIdxBench for NibbleOptBench {
    fn run(&self, st: &Structures) {
        let mut it = st.ntrie_opt.iter_last();
        if let Some(i) = it.current_index() { black_box(i); }
        while let Some(i) = it.prev_index() { black_box(i); }
    }
}
impl OptimizeBench for NibbleOptBench {
    fn run(&self, keys: &[Vec<u8>]) {
        let mut m = NT::trie_new();
        for (i, k) in keys.iter().enumerate() { m.trie_insert(k.clone(), i); }
        m.trie_optimize();
        black_box(&m);
    }
}
impl MemBench for NibbleOptBench {
    fn run(&self, keys: &[Vec<u8>]) -> u64 {
        let before = read_allocated();
        let mut m = NT::trie_new();
        for (i, k) in keys.iter().enumerate() { m.trie_insert(k.clone(), i); }
        m.trie_optimize();
        let bytes = read_allocated() - before;
        drop(m);
        bytes
    }
}

// PolyOpt (optimized PolyTrie)

impl LookupBench for PolyOptBench {
    fn run(&self, st: &Structures) {
        for k in &st.lookup_keys_null { black_box(st.ptrie_opt.trie_get(k)); }
    }
}
impl FwdIterBench for PolyOptBench {
    fn run(&self, st: &Structures) { st.ptrie_opt.trie_iter_fwd(|k, v| { black_box(k); black_box(v); }); }
}
impl RevIterBench for PolyOptBench {
    fn run(&self, st: &Structures) { st.ptrie_opt.trie_iter_rev(|k, v| { black_box(k); black_box(v); }); }
}
impl OptimizeBench for PolyOptBench {
    fn run(&self, keys: &[Vec<u8>]) {
        let mut m = PolyTrie::trie_new();
        for (i, k) in keys.iter().enumerate() { m.trie_insert(k.clone(), i); }
        m.trie_optimize();
        black_box(&m);
    }
}
impl MemBench for PolyOptBench {
    fn run(&self, keys: &[Vec<u8>]) -> u64 {
        let before = read_allocated();
        let mut m: PolyTrie<usize> = PolyTrie::trie_new();
        for (i, k) in keys.iter().enumerate() { m.trie_insert(k.clone(), i); }
        m.trie_optimize();
        let bytes = read_allocated() - before;
        drop(m);
        bytes
    }
}

// LinkedList

impl InsertBench for LinkedListBench {
    fn run(&self, keys: &[Vec<u8>]) {
        let mut list: LinkedList<(Vec<u8>, usize)> = LinkedList::new();
        for (i, k) in keys.iter().enumerate() { list.push_back((k.clone(), i)); }
        black_box(&list);
    }
}
impl LookupBench for LinkedListBench {
    fn run(&self, st: &Structures) {
        for k in &st.lookup_keys { black_box(st.llist.iter().find(|(key, _)| key == k)); }
    }
}
impl FwdIterBench for LinkedListBench {
    fn run(&self, st: &Structures) { for (k, v) in st.llist.iter() { black_box(k); black_box(v); } }
}
impl RevIterBench for LinkedListBench {
    fn run(&self, st: &Structures) { for (k, v) in st.llist.iter().rev() { black_box(k); black_box(v); } }
}
impl MemBench for LinkedListBench {
    fn run(&self, keys: &[Vec<u8>]) -> u64 {
        let before = read_allocated();
        let mut m: LinkedList<(Vec<u8>, usize)> = LinkedList::new();
        for (i, k) in keys.iter().enumerate() { m.push_back((k.clone(), i)); }
        let bytes = read_allocated() - before;
        drop(m);
        bytes
    }
}

// NibbleUnchecked (unsafe get_unchecked on unoptimized trie)
// Uses only hit_keys — get_unchecked assumes the key is in the set.

impl LookupBench for NibbleUncheckedBench {
    fn run(&self, st: &Structures) {
        for k in &st.hit_keys { black_box(unsafe { st.ntrie.get_unchecked(k) }); }
    }
}

// NibbleOptUnchecked (unsafe get_unchecked on optimized trie)

impl LookupBench for NibbleOptUncheckedBench {
    fn run(&self, st: &Structures) {
        for k in &st.hit_keys { black_box(unsafe { st.ntrie_opt.get_unchecked(k) }); }
    }
}

// DynNibbleTrie

impl InsertBench for DynNibbleTrieBench {
    fn run(&self, keys: &[Vec<u8>]) {
        let mut m = DynNibbleTrie::new();
        for (i, k) in keys.iter().enumerate() { m.insert(k.clone(), i).unwrap(); }
        black_box(&m);
    }
}
impl LookupBench for DynNibbleTrieBench {
    fn run(&self, st: &Structures) {
        for k in &st.lookup_keys { black_box(st.dyn_ntrie.get(k)); }
    }
}
impl FwdIterBench for DynNibbleTrieBench {
    fn run(&self, st: &Structures) { st.dyn_ntrie.iter_fwd(&mut |k, v| { black_box(k); black_box(v); }); }
}
impl RevIterBench for DynNibbleTrieBench {
    fn run(&self, st: &Structures) { st.dyn_ntrie.iter_rev(&mut |k, v| { black_box(k); black_box(v); }); }
}
impl MemBench for DynNibbleTrieBench {
    fn run(&self, keys: &[Vec<u8>]) -> u64 {
        let before = read_allocated();
        let mut m: DynNibbleTrie<usize> = DynNibbleTrie::new();
        for (i, k) in keys.iter().enumerate() { m.insert(k.clone(), i).unwrap(); }
        let bytes = read_allocated() - before;
        drop(m);
        bytes
    }
}

// DynNibbleOpt

impl LookupBench for DynNibbleOptBench {
    fn run(&self, st: &Structures) {
        for k in &st.lookup_keys { black_box(st.dyn_ntrie_opt.get(k)); }
    }
}
impl FwdIterBench for DynNibbleOptBench {
    fn run(&self, st: &Structures) { st.dyn_ntrie_opt.iter_fwd(&mut |k, v| { black_box(k); black_box(v); }); }
}
impl RevIterBench for DynNibbleOptBench {
    fn run(&self, st: &Structures) { st.dyn_ntrie_opt.iter_rev(&mut |k, v| { black_box(k); black_box(v); }); }
}
impl OptimizeBench for DynNibbleOptBench {
    fn run(&self, keys: &[Vec<u8>]) {
        let mut m = DynNibbleTrie::new();
        for (i, k) in keys.iter().enumerate() { m.insert(k.clone(), i).unwrap(); }
        m.optimize();
        black_box(&m);
    }
}
impl MemBench for DynNibbleOptBench {
    fn run(&self, keys: &[Vec<u8>]) -> u64 {
        let before = read_allocated();
        let mut m: DynNibbleTrie<usize> = DynNibbleTrie::new();
        for (i, k) in keys.iter().enumerate() { m.insert(k.clone(), i).unwrap(); }
        m.optimize();
        let bytes = read_allocated() - before;
        drop(m);
        bytes
    }
}

// ── Formatting ──────────────────────────────────────────────────────

fn fmt_rate(rate: f64) -> String {
    if rate >= 1e9 {
        format!("{:.2}G", rate / 1e9)
    } else if rate >= 1e6 {
        format!("{:.2}M", rate / 1e6)
    } else if rate >= 1e3 {
        format!("{:.1}K", rate / 1e3)
    } else {
        format!("{:.1}", rate)
    }
}

fn fmt_bytes_per(bytes: f64) -> String {
    if bytes >= 1e3 { format!("{:.0}", bytes) } else { format!("{:.1}", bytes) }
}

// ── Result storage ──────────────────────────────────────────────────

type ResultMap = HashMap<String, Vec<f64>>;

// ── Table printer ───────────────────────────────────────────────────

fn fmt_table(title: &str, unit: &str, data: &ResultMap, sizes: &[usize], names: &[&str], fmt_val: fn(f64) -> String, higher_is_better: bool) -> String {
    let active_names: Vec<&str> = names.iter().filter(|n| data.contains_key(*n as &str)).copied().collect();
    if active_names.is_empty() { return String::new(); }
    // Sort by first-column value — best first (descending for rates, ascending for memory).
    let mut sorted: Vec<&&str> = active_names.iter().collect();
    sorted.sort_by(|a, b| {
        let va = data.get(**a as &str).and_then(|v| v.first()).unwrap_or(&0.0);
        let vb = data.get(**b as &str).and_then(|v| v.first()).unwrap_or(&0.0);
        if higher_is_better { vb.partial_cmp(va) } else { va.partial_cmp(vb) }
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut s = format!("\n─── {title} ({unit}) ───\n");
    s.push_str(&format!("{:>COL$}", ""));
    for &sz in sizes { s.push_str(&format!("{:>COL$}", sz)); }
    s.push('\n');
    for name in &sorted {
        s.push_str(&format!("{:>COL$}", name));
        for &val in data.get(**name as &str).unwrap() { s.push_str(&format!("{:>COL$}", fmt_val(val))); }
        s.push('\n');
    }
    s
}

fn print_table(title: &str, unit: &str, data: &ResultMap, sizes: &[usize], names: &[&str]) {
    let s = fmt_table(title, unit, data, sizes, names, fmt_rate, true);
    if !s.is_empty() { print!("{s}"); }
}

fn print_mem_table(data: &ResultMap, sizes: &[usize], names: &[&str]) {
    let s = fmt_table("Memory", "bytes/key", data, sizes, names, fmt_bytes_per, false);
    if !s.is_empty() { print!("{s}"); }
}

// ── Persistent results ───────────────────────────────────────────────

#[derive(Serialize, Deserialize, Default)]
struct ResultsFile {
    sizes: Vec<usize>,
    #[serde(default)]
    sections: BTreeMap<String, BTreeMap<String, Vec<f64>>>,
}

const RESULTS_JSON: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/benches/bench_results.json");
const RESULTS_MD: &str   = concat!(env!("CARGO_MANIFEST_DIR"), "/benches/bench_results.md");

fn load_results() -> ResultsFile {
    match std::fs::read_to_string(RESULTS_JSON) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => ResultsFile::default(),
    }
}

fn save_results(data: &ResultsFile) {
    // JSON
    let json = serde_json::to_string_pretty(data).unwrap();
    std::fs::write(RESULTS_JSON, &json).unwrap();
    eprintln!("  wrote {RESULTS_JSON}");

    // Markdown — rows sorted by first-column value (fastest first for rates, smallest first for memory).
    // Collect all sizes that appear in any row across all sections, sorted.
    let mut all_sizes: Vec<usize> = Vec::new();
    for (_, rows) in &data.sections {
        for vals in rows.values() {
            for (i, &sz) in data.sizes.iter().enumerate() {
                if i < vals.len() && !all_sizes.contains(&sz) {
                    all_sizes.push(sz);
                }
            }
        }
    }
    all_sizes.sort();

    let mut md = String::new();
    for (section, rows) in &data.sections {
        let is_mem = section.contains("Memory");
        let fmt: fn(f64) -> String = if is_mem { fmt_bytes_per } else { fmt_rate };
        // Sort: for rate metrics higher is better (descending), for memory lower is better (ascending).
        let mut entries: Vec<_> = rows.iter().collect();
        if is_mem {
            entries.sort_by(|a, b| a.1.first().partial_cmp(&b.1.first()).unwrap_or(std::cmp::Ordering::Equal));
        } else {
            entries.sort_by(|a, b| b.1.first().partial_cmp(&a.1.first()).unwrap_or(std::cmp::Ordering::Equal));
        }
        md.push_str(&format!("\n─── {section} ───\n"));
        md.push_str(&format!("{:>COL$}", ""));
        for &sz in &all_sizes { md.push_str(&format!("{:>COL$}", sz)); }
        md.push('\n');
        for (name, vals) in entries {
            md.push_str(&format!("{:>COL$}", name));
            for (i, &sz) in all_sizes.iter().enumerate() {
                // Find the index of this size in data.sizes to get the right value.
                if let Some(pos) = data.sizes.iter().position(|&s| s == sz) {
                    if pos < vals.len() {
                        md.push_str(&format!("{:>COL$}", fmt(vals[pos])));
                    } else {
                        md.push_str(&format!("{:>COL$}", ""));
                    }
                } else {
                    md.push_str(&format!("{:>COL$}", ""));
                }
            }
            md.push('\n');
        }
    }
    if !md.is_empty() { md.push('\n'); }
    std::fs::write(RESULTS_MD, &md).unwrap();
    eprintln!("  wrote {RESULTS_MD}");
}

/// Merge new bench results into the persistent data.
///
/// Each section maps contestant names to a Vec<f64> aligned to `data.sizes`.
/// When merging, we only update entries for sizes that were actually run:
/// - If a contestant already exists, we splice in the new values at the
///   correct size positions, preserving any sizes that weren't run.
/// - If a contestant is new, we insert a full row (with gaps for sizes not run).
fn merge_results(data: &mut ResultsFile, section: &str, new: &ResultMap, run_sizes: &[usize]) {
    let sec = data.sections.entry(section.to_string()).or_default();
    for (name, values) in new {
        // values[i] corresponds to run_sizes[i]
        if let Some(existing) = sec.get_mut(name) {
            // Merge: update positions for the sizes we just ran
            for (i, &sz) in run_sizes.iter().enumerate() {
                if let Some(pos) = data.sizes.iter().position(|&s| s == sz) {
                    if i < values.len() {
                        // Extend if needed
                        while existing.len() <= pos { existing.push(0.0); }
                        existing[pos] = values[i];
                    }
                }
            }
        } else {
            // New contestant — create a full row aligned to data.sizes
            let mut row = vec![0.0; data.sizes.len()];
            for (i, &sz) in run_sizes.iter().enumerate() {
                if let Some(pos) = data.sizes.iter().position(|&s| s == sz) {
                    if i < values.len() && pos < row.len() {
                        row[pos] = values[i];
                    }
                }
            }
            sec.insert(name.clone(), row);
        }
    }
}

// ── Filter helper ────────────────────────────────────────────────────

/// Whether a contestant should run at the given size.
fn runnable(c: &Contestant, i: usize, active: &[bool], size: usize) -> bool {
    active[i] && c.max_size.map_or(true, |m| size <= m)
}

// ── Main ────────────────────────────────────────────────────────────

fn parse_args(args: &[String]) -> (Vec<String>, Vec<usize>, Vec<String>) {
    // Usage: bench [TESTS] [SIZES] [STRUCTURES]
    //   TESTS       – comma-list of test names (insert,lookup,fwd,rev,fwd_idx,rev_idx,optimize,memory)
    //   SIZES       – comma-list of sizes (10,100,1000,100000,10000000)
    //   STRUCTURES  – comma-list of structure name substrings (NibbleTrie,BTreeMap,…)
    // Any arg may be omitted or empty to mean "all".
    // Examples:
    //   bench                               – all tests, all sizes, all structures
    //   bench lookup 10,100                – lookup only, sizes 10 and 100, all structures
    //   bench "" 10                        – all tests, size 10 only, all structures
    //   bench lookup "" NibbleTrie         – lookup only, all sizes, only NibbleTrie variants
    //   bench "" "" Unchecked              – all tests, all sizes, only Unchecked variants
    //   bench lookup 1000 NibbleTrie       – lookup at 1K, NibbleTrie only
    let all_tests = ["insert", "lookup", "fwd", "rev", "fwd_idx", "rev_idx", "optimize", "memory"];
    let all_sizes = SIZES.to_vec();

    let positional: Vec<String> = args.iter().skip(1)
        .filter(|a| !a.starts_with("--"))
        .cloned()
        .collect();

    let tests_arg  = positional.get(0).map(|s| s.as_str()).unwrap_or("");
    let sizes_arg   = positional.get(1).map(|s| s.as_str()).unwrap_or("");
    let struct_arg  = positional.get(2).map(|s| s.as_str()).unwrap_or("");

    let tests: Vec<String> = if tests_arg.is_empty() {
        all_tests.iter().map(|t| t.to_string()).collect()
    } else {
        tests_arg.split(',')
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| !s.is_empty())
            .collect()
    };

    let sizes: Vec<usize> = if sizes_arg.is_empty() {
        all_sizes
    } else {
        sizes_arg.split(',')
            .filter_map(|s| s.trim().parse::<usize>().ok())
            .filter(|s| all_sizes.contains(s))
            .collect()
    };

    let struct_filters: Vec<String> = if struct_arg.is_empty() {
        Vec::new() // empty = all
    } else {
        struct_arg.split(',')
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| !s.is_empty())
            .collect()
    };

    (tests, sizes, struct_filters)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (tests, sizes, struct_filters) = parse_args(&args);

    let contestants = all_contestants();
    let active: Vec<bool> = contestants.iter().map(|c| {
        if struct_filters.is_empty() { true } else { struct_filters.iter().any(|f| c.name.to_ascii_lowercase().contains(f)) }
    }).collect();
    if active.iter().all(|a| !a) {
        let names: Vec<&str> = contestants.iter().map(|c| c.name).collect();
        eprintln!("No structures match filters {:?}. Available: {}", struct_filters, names.join(", "));
        std::process::exit(1);
    }

    let budget = Duration::from_secs(BENCH_SECS);
    let names: Vec<&str> = contestants.iter().map(|c| c.name).collect();

    // Resolve test enablement
    let run_insert   = tests.iter().any(|t| t == "insert");
    let run_lookup    = tests.iter().any(|t| t == "lookup");
    let run_fwd       = tests.iter().any(|t| t == "fwd");
    let run_rev       = tests.iter().any(|t| t == "rev");
    let run_fwd_idx   = tests.iter().any(|t| t == "fwd_idx");
    let run_rev_idx   = tests.iter().any(|t| t == "rev_idx");
    let run_optimize  = tests.iter().any(|t| t == "optimize");
    let run_memory    = tests.iter().any(|t| t == "memory");

    println!();
    println!("=== TinyTrie Benchmark Suite ===");
    {
        let active_names: Vec<&str> = names.iter().zip(active.iter()).filter(|(_, a)| **a).map(|(n, _)| *n).collect();
        eprintln!("Tests:    {}", tests.join(", "));
        eprintln!("Sizes:    {}", sizes.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(", "));
        eprintln!("Structs:  {}", if struct_filters.is_empty() { "all".to_string() } else { active_names.join(", ") });
    }
    println!("{BENCH_SECS}s per bench · sequential per size");
    println!();

    let mut ins  = ResultMap::new();
    let mut look = ResultMap::new();
    let mut fwd  = ResultMap::new();
    let mut rev  = ResultMap::new();
    let mut fwd_idx = ResultMap::new();
    let mut rev_idx = ResultMap::new();
    let mut mem  = ResultMap::new();
    let mut opt  = ResultMap::new();

    let mut results = load_results();
    // Merge run sizes into the stored sizes, preserving any sizes from previous runs.
    for &sz in &sizes {
        if !results.sizes.contains(&sz) {
            results.sizes.push(sz);
        }
    }
    results.sizes.sort();

    // Only build structures if any lookup/iteration test is needed.
    let needs_structures = run_lookup || run_fwd || run_rev || run_fwd_idx || run_rev_idx || run_memory;

    for &size in &sizes {
        eprintln!("[n = {size}]");

        // ── Generate keys ─────────────────────────────────────────────
        eprint!("  generating keys... ");
        let keys = string_keys(size);
        eprintln!("✓");

        // ── Insertion ──────────────────────────────────────────────────
        if run_insert {
            eprintln!("  insertion:");
            for (i, c) in contestants.iter().enumerate() {
                if !runnable(c, i, &active, size) { continue; }
                if let Some(ref b) = c.insert {
                    let r = bench(budget, c.name, || b.run(&keys));
                    ins.entry(c.name.into()).or_default().push(r.rate(size as u64));
                }
            }
        }

        // ── Build structures for lookup / iteration ─────────────────────
        let st = if needs_structures {
            eprint!("  building structures... ");
            let t0 = Instant::now();
            let st = build_all(&keys);
            eprintln!("{:.2}s ✓", t0.elapsed().as_secs_f64());
            Some(st)
        } else {
            None
        };

        // ── Lookup ───────────────────────────────────────────────────
        if run_lookup {
            eprintln!("  lookup:");
            let st = st.as_ref().unwrap();
            for (i, c) in contestants.iter().enumerate() {
                if !runnable(c, i, &active, size) { continue; }
                if let Some(ref b) = c.lookup {
                    // Unchecked variants iterate over hit_keys (known-present keys only);
                    // regular variants iterate over lookup_keys (hits + misses).
                    let ops = if c.name.contains("Unchecked") {
                        st.hit_keys.len()
                    } else {
                        st.lookup_keys.len()
                    };
                    let r = bench(budget, c.name, || b.run(st));
                    look.entry(c.name.into()).or_default().push(r.rate(ops as u64));
                }
            }
        }

        // ── Forward iteration ─────────────────────────────────────────
        if run_fwd {
            eprintln!("  iteration (forward):");
            let st = st.as_ref().unwrap();
            for (i, c) in contestants.iter().enumerate() {
                if !runnable(c, i, &active, size) { continue; }
                if let Some(ref b) = c.fwd_iter {
                    let r = bench(budget, c.name, || b.run(st));
                    fwd.entry(c.name.into()).or_default().push(r.rate(size as u64));
                }
            }
        }

        // ── Backward iteration ───────────────────────────────────────
        if run_rev {
            eprintln!("  iteration (backward):");
            let st = st.as_ref().unwrap();
            for (i, c) in contestants.iter().enumerate() {
                if !runnable(c, i, &active, size) { continue; }
                if let Some(ref b) = c.rev_iter {
                    let r = bench(budget, c.name, || b.run(st));
                    rev.entry(c.name.into()).or_default().push(r.rate(size as u64));
                }
            }
        }

        // ── Forward index iteration ──────────────────────────────────
        if run_fwd_idx {
            let any_fwd_idx = contestants.iter().zip(active.iter()).any(|(c, a)| *a && c.fwd_idx.is_some() && c.max_size.map_or(true, |m| size <= m));
            if any_fwd_idx {
                eprintln!("  iteration (forward index):");
                let st = st.as_ref().unwrap();
                for (i, c) in contestants.iter().enumerate() {
                    if !runnable(c, i, &active, size) { continue; }
                    if let Some(ref b) = c.fwd_idx {
                        let r = bench(budget, c.name, || b.run(st));
                        fwd_idx.entry(c.name.into()).or_default().push(r.rate(size as u64));
                    }
                }
            }
        }

        // ── Backward index iteration ──────────────────────────────────
        if run_rev_idx {
            let any_rev_idx = contestants.iter().zip(active.iter()).any(|(c, a)| *a && c.rev_idx.is_some() && c.max_size.map_or(true, |m| size <= m));
            if any_rev_idx {
                eprintln!("  iteration (backward index):");
                let st = st.as_ref().unwrap();
                for (i, c) in contestants.iter().enumerate() {
                    if !runnable(c, i, &active, size) { continue; }
                    if let Some(ref b) = c.rev_idx {
                        let r = bench(budget, c.name, || b.run(st));
                        rev_idx.entry(c.name.into()).or_default().push(r.rate(size as u64));
                    }
                }
            }
        }

        // ── Optimize time ─────────────────────────────────────────────
        if run_optimize {
            let any_opt = contestants.iter().zip(active.iter()).any(|(c, a)| *a && c.optimize.is_some() && c.max_size.map_or(true, |m| size <= m));
            if any_opt {
                eprintln!("  optimize:");
                for (i, c) in contestants.iter().enumerate() {
                    if !runnable(c, i, &active, size) { continue; }
                    if let Some(ref b) = c.optimize {
                        let r = bench(budget, c.name, || b.run(&keys));
                        opt.entry(c.name.into()).or_default().push(r.rate(size as u64));
                    }
                }
            }
        }

        // ── Memory (sequential, needs clean allocator state) ────────
        drop(st);
        if run_memory {
            eprintln!("  memory:");
            for (i, c) in contestants.iter().enumerate() {
                if !runnable(c, i, &active, size) { continue; }
                if let Some(ref b) = c.mem {
                    let bytes = b.run(&keys);
                    eprintln!("    {}: {bytes} bytes ({:.1}/key)", c.name, bytes as f64 / size as f64);
                    mem.entry(c.name.into()).or_default().push(bytes as f64 / size as f64);
                }
            }
        }

        eprintln!();
    }

    // ── Print summary tables ────────────────────────────────────────

    if run_insert    { print_table("Insertion", "keys/sec", &ins, &sizes, &names); }
    if run_lookup    { print_table("Lookup", "keys/sec", &look, &sizes, &names); }
    if run_fwd       { print_table("Iter forward", "keys/sec", &fwd, &sizes, &names); }
    if run_rev       { print_table("Iter backward", "keys/sec", &rev, &sizes, &names); }
    if run_fwd_idx   { print_table("Iter fwd index", "keys/sec", &fwd_idx, &sizes, &names); }
    if run_rev_idx   { print_table("Iter rev index", "keys/sec", &rev_idx, &sizes, &names); }
    if run_optimize  { print_table("Optimize", "keys/sec", &opt, &sizes, &names); }
    if run_memory    { print_mem_table(&mem, &sizes, &names); }

    // ── Merge and save results ───────────────────────────────────────
    if run_insert    { merge_results(&mut results, "Insertion (keys/sec)",  &ins, &sizes); }
    if run_lookup    { merge_results(&mut results, "Lookup (keys/sec)",     &look, &sizes); }
    if run_fwd       { merge_results(&mut results, "Iter forward (keys/sec)",  &fwd, &sizes); }
    if run_rev       { merge_results(&mut results, "Iter backward (keys/sec)", &rev, &sizes); }
    if run_fwd_idx   { merge_results(&mut results, "Iter fwd index (keys/sec)", &fwd_idx, &sizes); }
    if run_rev_idx   { merge_results(&mut results, "Iter rev index (keys/sec)", &rev_idx, &sizes); }
    if run_optimize  { merge_results(&mut results, "Optimize (keys/sec)",  &opt, &sizes); }
    if run_memory    { merge_results(&mut results, "Memory (bytes/key)",   &mem, &sizes); }
    save_results(&results);

    println!();
}
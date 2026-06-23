use std::hint::black_box;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use clap::{Parser, ValueEnum};
use serde::{Deserialize, Serialize};
use tiny_trie::{NibbleTrie, Node, TrieIndex};

// ── bench_query_methods! macro (must precede mod declarations) ─────────
//
// Generates `bench_lookup`, `bench_fwd_iter`, `bench_rev_iter`,
// `bench_fwd_idx`, `bench_rev_idx`, and `lookup_ops` methods for
// `Benchable` impls.  Only the query-side methods; the struct must
// still manually implement `build`, plus any of `bench_insert`,
// `bench_optimize`, and `bench_memory`.
//
// Usage:
//   bench_query_methods! {
//       field: trie,
//       lookup: get(lookup),           // trie_get|get|get_unchecked × lookup|null|truncated|hit
//       fwd_iter: iter_kv,             // trie_callback|dyn_callback|iter_kv|iter_kv_no_current|none
//       rev_iter: iter_kv,             // trie_callback|dyn_callback|iter_kv|iter_kv_no_current|none
//       index_iter: true,              // optional, default false
//       unchecked: true,               // optional, default false — overrides lookup_ops to hit_keys
//   }

macro_rules! bench_query_methods {
    // ── Top-level entry: parse the spec and dispatch ────────────────
    // Full form: with index_iter and unchecked
    (
        field: $field:ident,
        lookup: $lookup_method:ident($key_set:ident),
        fwd_iter: $fwd_style:ident,
        rev_iter: $rev_style:ident,
        index_iter: $idx:tt,
        unchecked: $unchecked:tt,
    ) => {
        bench_query_methods!(@lookup $field, $lookup_method, $key_set);
        bench_query_methods!(@fwd $field, $fwd_style);
        bench_query_methods!(@rev $field, $rev_style);
        bench_query_methods!(@idx $field, $idx);
        bench_query_methods!(@ops $unchecked);
    };
    // With index_iter, without unchecked
    (
        field: $field:ident,
        lookup: $lookup_method:ident($key_set:ident),
        fwd_iter: $fwd_style:ident,
        rev_iter: $rev_style:ident,
        index_iter: $idx:tt,
    ) => {
        bench_query_methods!(@lookup $field, $lookup_method, $key_set);
        bench_query_methods!(@fwd $field, $fwd_style);
        bench_query_methods!(@rev $field, $rev_style);
        bench_query_methods!(@idx $field, $idx);
        bench_query_methods!(@ops);
    };
    // Without index_iter, with unchecked
    (
        field: $field:ident,
        lookup: $lookup_method:ident($key_set:ident),
        fwd_iter: $fwd_style:ident,
        rev_iter: $rev_style:ident,
        unchecked: $unchecked:tt,
    ) => {
        bench_query_methods!(@lookup $field, $lookup_method, $key_set);
        bench_query_methods!(@fwd $field, $fwd_style);
        bench_query_methods!(@rev $field, $rev_style);
        bench_query_methods!(@idx $field);
        bench_query_methods!(@ops $unchecked);
    };
    // Minimal form: no index_iter, no unchecked
    (
        field: $field:ident,
        lookup: $lookup_method:ident($key_set:ident),
        fwd_iter: $fwd_style:ident,
        rev_iter: $rev_style:ident,
    ) => {
        bench_query_methods!(@lookup $field, $lookup_method, $key_set);
        bench_query_methods!(@fwd $field, $fwd_style);
        bench_query_methods!(@rev $field, $rev_style);
        bench_query_methods!(@idx $field);
        bench_query_methods!(@ops);
    };

    // ── Lookup ────────────────────────────────────────────────────────

    (@lookup $field:ident, trie_get, lookup) => {
        fn bench_lookup(&self, ctx: &BenchContext) -> Option<()> {
            for k in &ctx.lookup_keys { std::hint::black_box(self.$field.trie_get(k)); }
            Some(())
        }
    };
    (@lookup $field:ident, trie_get, null) => {
        fn bench_lookup(&self, ctx: &BenchContext) -> Option<()> {
            for k in &ctx.lookup_keys_null { std::hint::black_box(self.$field.trie_get(k)); }
            Some(())
        }
    };
    (@lookup $field:ident, get, lookup) => {
        fn bench_lookup(&self, ctx: &BenchContext) -> Option<()> {
            for k in &ctx.lookup_keys { std::hint::black_box(self.$field.get(k)); }
            Some(())
        }
    };
    (@lookup $field:ident, get, null) => {
        fn bench_lookup(&self, ctx: &BenchContext) -> Option<()> {
            for k in &ctx.lookup_keys_null { std::hint::black_box(self.$field.get(k)); }
            Some(())
        }
    };
    (@lookup $field:ident, get, truncated) => {
        fn bench_lookup(&self, ctx: &BenchContext) -> Option<()> {
            for k in &ctx.fl_lookup_keys { std::hint::black_box(self.$field.get(k)); }
            Some(())
        }
    };
    (@lookup $field:ident, get_unchecked, hit) => {
        fn bench_lookup(&self, ctx: &BenchContext) -> Option<()> {
            for k in &ctx.hit_keys { std::hint::black_box(unsafe { self.$field.get_unchecked(k) }); }
            Some(())
        }
    };

    // ── Forward iteration ──────────────────────────────────────────────

    (@fwd $field:ident, trie_callback) => {
        fn bench_fwd_iter(&self) -> Option<()> {
            self.$field.trie_iter_fwd(|k, v| { std::hint::black_box(k); std::hint::black_box(v); });
            Some(())
        }
    };
    (@fwd $field:ident, dyn_callback) => {
        fn bench_fwd_iter(&self) -> Option<()> {
            self.$field.iter_fwd(&mut |k, v| { std::hint::black_box(k); std::hint::black_box(v); });
            Some(())
        }
    };
    (@fwd $field:ident, iter_kv) => {
        fn bench_fwd_iter(&self) -> Option<()> {
            let mut it = self.$field.iter();
            if let Some((k, v)) = it.current() { std::hint::black_box(k); std::hint::black_box(v); }
            while let Some((k, v)) = it.next() { std::hint::black_box(k); std::hint::black_box(v); }
            Some(())
        }
    };
    (@fwd $field:ident, iter_kv_no_current) => {
        fn bench_fwd_iter(&self) -> Option<()> {
            let mut it = self.$field.iter();
            while let Some((k, v)) = it.next() { std::hint::black_box(k); std::hint::black_box(v); }
            Some(())
        }
    };
    (@fwd $field:ident, none) => {
        // no bench_fwd_iter
    };

    // ── Reverse iteration ──────────────────────────────────────────────

    (@rev $field:ident, trie_callback) => {
        fn bench_rev_iter(&self) -> Option<()> {
            self.$field.trie_iter_rev(|k, v| { std::hint::black_box(k); std::hint::black_box(v); });
            Some(())
        }
    };
    (@rev $field:ident, dyn_callback) => {
        fn bench_rev_iter(&self) -> Option<()> {
            self.$field.iter_rev(&mut |k, v| { std::hint::black_box(k); std::hint::black_box(v); });
            Some(())
        }
    };
    (@rev $field:ident, iter_kv) => {
        fn bench_rev_iter(&self) -> Option<()> {
            let mut it = self.$field.iter_last();
            if let Some((k, v)) = it.current() { std::hint::black_box(k); std::hint::black_box(v); }
            while let Some((k, v)) = it.prev() { std::hint::black_box(k); std::hint::black_box(v); }
            Some(())
        }
    };
    (@rev $field:ident, iter_kv_no_current) => {
        fn bench_rev_iter(&self) -> Option<()> {
            let mut it = self.$field.iter_last();
            while let Some((k, v)) = it.prev() { std::hint::black_box(k); std::hint::black_box(v); }
            Some(())
        }
    };
    (@rev $field:ident, none) => {
        // no bench_rev_iter
    };

    // ── Index iteration (optional) ──────────────────────────────────

    (@idx $field:ident, true) => {
        fn bench_fwd_idx(&self) -> Option<()> {
            let mut it = self.$field.iter();
            if let Some(i) = it.current_index() { std::hint::black_box(i); }
            while let Some(i) = it.next_index() { std::hint::black_box(i); }
            Some(())
        }
        fn bench_rev_idx(&self) -> Option<()> {
            let mut it = self.$field.iter_last();
            if let Some(i) = it.current_index() { std::hint::black_box(i); }
            while let Some(i) = it.prev_index() { std::hint::black_box(i); }
            Some(())
        }
    };
    (@idx $field:ident) => {
        // no index iteration
    };
    (@idx $field:ident, false) => {
        // no index iteration
    };

    // ── lookup_ops override (unchecked variants) ─────────────────────

    (@ops true) => {
        fn lookup_ops(&self, ctx: &BenchContext) -> usize { ctx.hit_keys.len() }
    };
    (@ops false) => {
        // use default: ctx.lookup_keys.len()
    };
    (@ops) => {
        // use default: ctx.lookup_keys.len()
    };
}

mod keygen;
mod results;

mod bit_trie;
mod dyn_trie;
mod fixed_len;
mod nibble_trie;
mod poly_trie;
mod stacked_trie;
mod std_contestants;
mod tiny_btree;

// ── Re-exports from modules ──────────────────────────────────────────

use keygen::*;
use results::*;

use bit_trie::BitTrieBench;
use dyn_trie::{DynTrieBench, DynTrieOptBench};
use fixed_len::{FixedLenBench, FixedLenOptBench};
use nibble_trie::{NibbleOptBench, NibbleOptUncheckedBench, NibbleTrieBench, NibbleUncheckedBench};
use poly_trie::{PolyOptBench, PolyTrieBench};
use stacked_trie::{StackedTrie2Bench, StackedTrie4Bench};
use std_contestants::{BTreeMapBench, HashMapBench, LinkedListBench, SortedVecBench};
use tiny_btree::{CTreeBench, CTreeOptBench};

// ── Type aliases ─────────────────────────────────────────────────────

type NT = NibbleTrie<usize, u32, u32>;
type NT2 = NibbleTrie<usize, u32, u32, 2>;
type NT4 = NibbleTrie<usize, u32, u32, 4>;

// ── Config ───────────────────────────────────────────────────────────

const COL: usize = 16;
const NAME_COL: usize = 22;

// ── Allocation tracker ──────────────────────────────────────────────

#[global_allocator]
static TRACKER: TrackingAllocator = TrackingAllocator;

struct TrackingAllocator;
static ALLOCATED: AtomicU64 = AtomicU64::new(0);

unsafe impl std::alloc::GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        ALLOCATED.fetch_add(layout.size() as u64, Ordering::Relaxed);
        unsafe { std::alloc::System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
        ALLOCATED.fetch_sub(layout.size() as u64, Ordering::Relaxed);
        unsafe { std::alloc::System.dealloc(ptr, layout) }
    }
}

pub(crate) fn read_allocated() -> u64 {
    ALLOCATED.load(Ordering::Relaxed)
}

// ── Stacked-trie conversion ──────────────────────────────────────────

/// Convert a STAK=1 NibbleTrie to a STAK=N trie (1:1 vnode mapping).
/// Internal child addresses are remapped: phys → phys * DST_STAK.
pub(crate) fn convert_stak1_to<const DST_STAK: usize>(src: &NT) -> NibbleTrie<usize, u32, u32, DST_STAK> {
    let mut dst: NibbleTrie<usize, u32, u32, DST_STAK> = NibbleTrie::new();
    dst.buf = src.buf.clone();
    dst.index = src.index.clone();
    dst.values = src.values.clone();
    for node1 in &src.arena {
        let mut node_dst: Node<u32, u32, DST_STAK> = Node::new();
        for nib in 0..16 {
            if node1.is_occupied(nib, 0) {
                if node1.is_leaf(nib, 0) {
                    node_dst.children[nib] = node1.children[nib];
                } else {
                    node_dst.children[nib] = u32::from_usize(node1.children[nib].as_usize() * DST_STAK);
                }
                node_dst.occupancy[0] |= 1 << nib;
                if node1.is_leaf(nib, 0) {
                    node_dst.leaf_mask[0] |= 1 << nib;
                }
            }
        }
        node_dst.prefix_len[0] = node1.prefix_len[0];
        node_dst.leaf = node1.leaf;
        node_dst.terminal = if node1.is_terminal(0) { 1 } else { 0 };
        dst.arena.push(node_dst);
    }
    dst
}

// ── FixedLen helpers ────────────────────────────────────────────────

pub(crate) const FIXED_LEN_MAX: usize = 16;

pub(crate) fn truncate_key(key: &[u8]) -> Vec<u8> {
    if key.len() <= FIXED_LEN_MAX { key.to_vec() } else { key[..FIXED_LEN_MAX].to_vec() }
}

pub(crate) fn max_key_len(keys: &[Vec<u8>]) -> usize {
    keys.iter().map(|k| k.len().min(FIXED_LEN_MAX)).max().unwrap_or(1)
}

// ── Sorted-vec helpers ──────────────────────────────────────────────

pub(crate) fn build_sorted_vec(keys: &[Vec<u8>]) -> Vec<(Vec<u8>, usize)> {
    let mut v: Vec<_> = keys.iter().enumerate().map(|(i, k)| (k.clone(), i)).collect();
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v
}

pub(crate) fn sorted_vec_get(sv: &[(Vec<u8>, usize)], key: &[u8]) -> Option<usize> {
    sv.binary_search_by(|e| e.0.as_slice().cmp(key)).ok().map(|i| sv[i].1)
}

// ── BenchContext ─────────────────────────────────────────────────────

/// Shared key sets for lookup benchmarks — built once per size.
pub(crate) struct BenchContext {
    pub lookup_keys: Vec<Vec<u8>>,
    pub lookup_keys_null: Vec<Vec<u8>>,
    pub fl_lookup_keys: Vec<Vec<u8>>,
    pub hit_keys: Vec<Vec<u8>>,
}

fn build_context(keys: &[Vec<u8>]) -> BenchContext {
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
    let fl_lookup_keys: Vec<Vec<u8>> = lookup_keys.iter().map(|k| truncate_key(k)).collect();
    BenchContext {
        hit_keys: keys.to_vec(),
        lookup_keys,
        lookup_keys_null,
        fl_lookup_keys,
    }
}

// ── Benchable trait ─────────────────────────────────────────────────

pub(crate) trait Benchable {
    /// Populate internal state from keys. Called once per size before query benches.
    fn build(&mut self, _keys: &[Vec<u8>], _ctx: &BenchContext) {}

    fn bench_insert(&self, _keys: &[Vec<u8>]) -> Option<()> { None }
    fn bench_lookup(&self, _ctx: &BenchContext) -> Option<()> { None }
    fn bench_fwd_iter(&self) -> Option<()> { None }
    fn bench_rev_iter(&self) -> Option<()> { None }
    fn bench_fwd_idx(&self) -> Option<()> { None }
    fn bench_rev_idx(&self) -> Option<()> { None }
    fn bench_optimize(&self, _keys: &[Vec<u8>]) -> Option<()> { None }
    fn bench_memory(&self, _keys: &[Vec<u8>]) -> Option<f64> { None }

    /// Number of lookup operations — overridden by unchecked variants.
    fn lookup_ops(&self, ctx: &BenchContext) -> usize { ctx.lookup_keys.len() }

    /// Which key domain this contestant requires. `Any` = all key modes.
    /// `Strings` = skip for key modes that may contain null bytes.
    fn key_domain(&self) -> KeyDomain { KeyDomain::Any }

    /// Inform the contestant of the active key mode. Called once before the
    /// size loop. The default is a no-op — most contestants are mode-agnostic
    /// and ignore this. Contestants whose internal key type depends on the
    /// domain (e.g. `CTreeBench`, which keys on `u64` for fixed-width modes and
    /// `Box<[u8]>` for variable modes) override it so `bench_insert`/`build`,
    /// which don't all receive the context, can pick the right key type.
    fn set_key_mode(&mut self, _mode: KeyMode) {}
}


// ── Contestant ────────────────────────────────────────────────────────

struct Contestant {
    name: &'static str,
    /// Skip this contestant for sizes larger than this (None = no limit).
    max_size: Option<usize>,
    bench: Box<dyn Benchable>,
}

/// Whether a contestant should run at the given size.
fn runnable(c: &Contestant, i: usize, active: &[bool], size: usize) -> bool {
    active[i] && c.max_size.map_or(true, |m| size <= m)
}

fn all_contestants() -> Vec<Contestant> {
    vec![
        Contestant { name: "NibbleTrie",        max_size: None, bench: Box::new(NibbleTrieBench::new()) },
        Contestant { name: "BitTrie",            max_size: None, bench: Box::new(BitTrieBench::new()) },
        Contestant { name: "BTreeMap",           max_size: None, bench: Box::new(BTreeMapBench::new()) },
        Contestant { name: "HashMap",            max_size: None, bench: Box::new(HashMapBench::new()) },
        Contestant { name: "SortedVec",          max_size: None, bench: Box::new(SortedVecBench::new()) },
        Contestant { name: "NibbleOpt",         max_size: None, bench: Box::new(NibbleOptBench::new()) },
        Contestant { name: "LinkedList",         max_size: None, bench: Box::new(LinkedListBench::new()) },
        Contestant { name: "NibbleUnchecked",    max_size: None, bench: Box::new(NibbleUncheckedBench::new()) },
        Contestant { name: "NibbleOptUnchecked", max_size: None, bench: Box::new(NibbleOptUncheckedBench::new()) },
        Contestant { name: "DynTrie",            max_size: None, bench: Box::new(DynTrieBench::new()) },
        Contestant { name: "DynTrieOpt",         max_size: None, bench: Box::new(DynTrieOptBench::new()) },
        Contestant { name: "PolyTrie",           max_size: None, bench: Box::new(PolyTrieBench::new()) },
        Contestant { name: "PolyOpt",            max_size: None, bench: Box::new(PolyOptBench::new()) },
        Contestant { name: "FixedLen",            max_size: None, bench: Box::new(FixedLenBench::new()) },
        Contestant { name: "FixedLenOpt",         max_size: None, bench: Box::new(FixedLenOptBench::new()) },
        Contestant { name: "StackedTrie2",        max_size: None, bench: Box::new(StackedTrie2Bench::new()) },
        Contestant { name: "StackedTrie4",        max_size: None, bench: Box::new(StackedTrie4Bench::new()) },
        Contestant { name: "CTree",               max_size: None, bench: Box::new(CTreeBench::new()) },
        Contestant { name: "CTreeOpt",            max_size: None, bench: Box::new(CTreeOptBench::new()) },
    ]
}

// ── Bench harness ─────────────────────────────────────────────────────

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
/// Returns `None` if the first call returns `None` (unsupported test).
fn bench(budget: Duration, label: &str, f: impl Fn() -> Option<()>) -> Option<BenchResult> {
    let mut iters = 0u64;
    let start = Instant::now();
    loop {
        f()?;
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
    Some(BenchResult { iters, elapsed })
}

// ── CLI ───────────────────────────────────────────────────────────────

const ALL_TESTS: &[&str] = &["insert", "lookup", "fwd", "rev", "fwd_idx", "rev_idx", "optimize", "memory"];

#[derive(Parser)]
#[command(name = "bencher", bin_name = "bencher")]
#[command(about = "TinyTrie Benchmark Suite")]
#[command(version)]
struct Cli {
    #[arg(long, short, value_delimiter = ',', default_values = ALL_TESTS)]
    tests: Vec<String>,
    #[arg(long, short)]
    sizes: Option<String>,
    #[arg(long, value_delimiter = ',')]
    structures: Option<Vec<String>>,
    #[arg(long, default_value = "sequential")]
    keys: KeyMode,
    #[arg(long)]
    corpus: Option<String>,
    #[arg(long, default_value_t = 2)]
    time: u64,
}

fn resolve_tests(raw: &[String]) -> Vec<String> {
    let mut resolved = Vec::new();
    let mut unknown = Vec::new();
    for s in raw {
        let lower = s.to_ascii_lowercase();
        let normalized = match lower.as_str() {
            "insert" | "insertion" => "insert",
            "lookup" => "lookup",
            "fwd" | "forward" => "fwd",
            "rev" | "backward" => "rev",
            "fwd_idx" | "forward_idx" | "forward_index" => "fwd_idx",
            "rev_idx" | "backward_idx" | "backward_index" | "rev_index" => "rev_idx",
            "optimize" | "opt" => "optimize",
            "memory" | "mem" => "memory",
            other => { unknown.push(other.to_string()); continue; }
        };
        if !resolved.iter().any(|t| t == normalized) {
            resolved.push(normalized.to_string());
        }
    }
    if !unknown.is_empty() {
        eprintln!("Error: unknown test(s): {}. Valid tests: {}", unknown.join(", "), ALL_TESTS.join(", "));
        std::process::exit(1);
    }
    resolved
}

// ── Main ──────────────────────────────────────────────────────────────

fn main() {
    let cli = Cli::parse();

    if matches!(cli.keys, KeyMode::Words | KeyMode::Lines) && cli.corpus.is_none() {
        eprintln!("Error: --keys={} requires --corpus <file>",
            match cli.keys { KeyMode::Words => "words", KeyMode::Lines => "lines", _ => unreachable!() });
        std::process::exit(1);
    }
    if matches!(cli.keys, KeyMode::RandomU64 | KeyMode::SeqU64) && cli.corpus.is_some() {
        eprintln!("Warning: --keys={}? ignores --corpus",
            match cli.keys { KeyMode::RandomU64 => "random-u64", KeyMode::SeqU64 => "seq-u64", _ => unreachable!() });
    }

    let tests = resolve_tests(&cli.tests);
    let mut sizes = resolve_sizes(cli.sizes.as_deref());
    let struct_filters: Vec<String> = match &cli.structures {
        Some(s) if !s.is_empty() => s.iter().map(|f| f.to_ascii_lowercase()).collect(),
        _ => Vec::new(),
    };
    let key_mode = cli.keys;
    let corpus_path = cli.corpus;
    let bench_secs = cli.time;

    let corpus_keys: Option<Vec<Vec<u8>>> = match key_mode {
        KeyMode::Words => {
            let path = corpus_path.as_deref().expect("--keys=words requires --corpus <file>");
            let all = load_corpus_words(path);
            eprintln!("Corpus: {} unique words from {}", all.len(), path);
            Some(all)
        }
        KeyMode::Lines => {
            let path = corpus_path.as_deref().expect("--keys=lines requires --corpus <file>");
            let all = load_corpus_lines(path);
            eprintln!("Corpus: {} unique lines from {}", all.len(), path);
            Some(all)
        }
        _ => None,
    };
    if let Some(ref all) = corpus_keys {
        let max_n = all.len();
        let before = sizes.len();
        sizes.retain(|&s| s <= max_n);
        if sizes.len() < before {
            let skipped: Vec<usize> = resolve_sizes(cli.sizes.as_deref())
                .into_iter().filter(|&s| s > max_n).collect();
            eprintln!("Skipping sizes {:?}: corpus has only {} entries", skipped, max_n);
            if sizes.is_empty() {
                eprintln!("Error: no sizes to benchmark (corpus has only {} entries, all requested sizes exceed it)", max_n);
                std::process::exit(1);
            }
        }
    }

    let mut contestants = all_contestants();
    let active: Vec<bool> = contestants.iter().map(|c| {
        if struct_filters.is_empty() { true } else { struct_filters.iter().any(|f| c.name.to_ascii_lowercase().contains(f)) }
    }).collect();
    if active.iter().all(|a| !a) {
        let names: Vec<&str> = contestants.iter().map(|c| c.name).collect();
        eprintln!("No structures match filters {:?}. Available: {}", struct_filters, names.join(", "));
        std::process::exit(1);
    }

    let budget = Duration::from_secs(bench_secs);
    let names: Vec<&str> = contestants.iter().map(|c| c.name).collect();

    // Inform each contestant of the active key mode once, before the size
    // loop. Mode-dependent contestants (e.g. `CTreeBench`) use this to pick
    // their key type; the rest ignore it via the default no-op.
    for c in contestants.iter_mut() {
        c.bench.set_key_mode(key_mode);
    }

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
        eprintln!("Keys:     {:?}", key_mode);
    }
    println!("{bench_secs}s per bench · sequential per size");
    println!();

    let mut ins  = ResultMap::new();
    let mut look = ResultMap::new();
    let mut fwd  = ResultMap::new();
    let mut rev  = ResultMap::new();
    let mut fwd_idx = ResultMap::new();
    let mut rev_idx = ResultMap::new();
    let mut mem  = ResultMap::new();
    let mut opt  = ResultMap::new();

    let (json_path, md_path) = results_paths(&key_mode);
    let mut results = load_results(&json_path);
    for &sz in &sizes {
        if !results.sizes.contains(&sz) {
            results.sizes.push(sz);
        }
    }
    results.sizes.sort();

    let needs_structures = run_lookup || run_fwd || run_rev || run_fwd_idx || run_rev_idx;

    // Pre-compute which contestants are incompatible with this key mode.
    //   Strings  — skip modes that may embed null bytes (null-terminator stores)
    //   Variable — skip fixed-width u64 modes (those belong to fixed-key stores)
    //
    //   `CTree` is NOT skipped here: it declares `Any` and picks its key type
    //   (`u64` vs `Box<[u8]>`) from the mode set via `set_key_mode`, so it runs
    //   in every mode with the comparison path matching that key type.
    let skip_for_keys: Vec<bool> = contestants.iter()
        .map(|c| {
            let d = c.bench.key_domain();
            (d == KeyDomain::Strings && key_mode.may_contain_null_bytes())
                || (d == KeyDomain::Variable && key_mode.is_fixed_width())
        })
        .collect();

    for &size in &sizes {
        eprintln!("[n = {size}]");

        eprint!("  generating keys ({:?})... ", key_mode);
        let keys = generate_keys(&key_mode, size, corpus_keys.as_deref());
        eprintln!("✓ ({} keys)", keys.len());

        // Announce skipped contestants for incompatible key modes
        for (i, c) in contestants.iter().enumerate() {
            if skip_for_keys[i] && runnable(c, i, &active, size) {
                eprintln!("  {}: skipped (incompatible key mode {:?})", c.name, key_mode);
            }
        }

        // ── Insertion ──────────────────────────────────────────────────
        if run_insert {
            eprintln!("  insertion:");
            for (i, c) in contestants.iter().enumerate() {
                if !runnable(c, i, &active, size) || skip_for_keys[i] { continue; }
                if let Some(r) = bench(budget, c.name, || c.bench.bench_insert(&keys)) {
                    ins.entry(c.name.into()).or_default().push(r.rate(size as u64));
                }
            }
        }

        // ── Build structures for lookup / iteration ─────────────────────
        let ctx = if needs_structures {
            eprint!("  building structures... ");
            let t0 = Instant::now();
            let ctx = build_context(&keys);
            for (i, c) in contestants.iter_mut().enumerate() {
                if skip_for_keys[i] || !runnable(c, i, &active, size) { continue; }
                c.bench.build(&keys, &ctx);
            }
            eprintln!("{:.2}s ✓", t0.elapsed().as_secs_f64());
            Some(ctx)
        } else {
            None
        };

        // ── Lookup ───────────────────────────────────────────────────
        if run_lookup {
            eprintln!("  lookup:");
            let ctx = ctx.as_ref().unwrap();
            for (i, c) in contestants.iter().enumerate() {
                if !runnable(c, i, &active, size) || skip_for_keys[i] { continue; }
                let ops = c.bench.lookup_ops(ctx);
                if let Some(r) = bench(budget, c.name, || c.bench.bench_lookup(ctx)) {
                    look.entry(c.name.into()).or_default().push(r.rate(ops as u64));
                }
            }
        }

        // ── Forward iteration ─────────────────────────────────────────
        if run_fwd {
            eprintln!("  iteration (forward):");
            for (i, c) in contestants.iter().enumerate() {
                if !runnable(c, i, &active, size) || skip_for_keys[i] { continue; }
                if let Some(r) = bench(budget, c.name, || c.bench.bench_fwd_iter()) {
                    fwd.entry(c.name.into()).or_default().push(r.rate(size as u64));
                }
            }
        }

        // ── Backward iteration ───────────────────────────────────────
        if run_rev {
            eprintln!("  iteration (backward):");
            for (i, c) in contestants.iter().enumerate() {
                if !runnable(c, i, &active, size) || skip_for_keys[i] { continue; }
                if let Some(r) = bench(budget, c.name, || c.bench.bench_rev_iter()) {
                    rev.entry(c.name.into()).or_default().push(r.rate(size as u64));
                }
            }
        }

        // ── Forward index iteration ──────────────────────────────────
        if run_fwd_idx {
            eprintln!("  iteration (forward index):");
            for (i, c) in contestants.iter().enumerate() {
                if !runnable(c, i, &active, size) || skip_for_keys[i] { continue; }
                if let Some(r) = bench(budget, c.name, || c.bench.bench_fwd_idx()) {
                    fwd_idx.entry(c.name.into()).or_default().push(r.rate(size as u64));
                }
            }
        }

        // ── Backward index iteration ──────────────────────────────────
        if run_rev_idx {
            eprintln!("  iteration (backward index):");
            for (i, c) in contestants.iter().enumerate() {
                if !runnable(c, i, &active, size) || skip_for_keys[i] { continue; }
                if let Some(r) = bench(budget, c.name, || c.bench.bench_rev_idx()) {
                    rev_idx.entry(c.name.into()).or_default().push(r.rate(size as u64));
                }
            }
        }

        // ── Optimize time ─────────────────────────────────────────────
        if run_optimize {
            eprintln!("  optimize:");
            for (i, c) in contestants.iter().enumerate() {
                if !runnable(c, i, &active, size) || skip_for_keys[i] { continue; }
                if let Some(r) = bench(budget, c.name, || c.bench.bench_optimize(&keys)) {
                    opt.entry(c.name.into()).or_default().push(r.rate(size as u64));
                }
            }
        }

        // ── Memory ────────────────────────────────────────────────────
        if run_memory {
            eprintln!("  memory:");
            for (i, c) in contestants.iter().enumerate() {
                if !runnable(c, i, &active, size) || skip_for_keys[i] { continue; }
                if let Some(bytes_per_key) = c.bench.bench_memory(&keys) {
                    eprintln!("    {}: {:.1}/key", c.name, bytes_per_key);
                    mem.entry(c.name.into()).or_default().push(bytes_per_key);
                }
            }
        }

        eprintln!();
    }

    // ── Print summary tables ────────────────────────────────────────
    if run_insert   { print_table("Insertion", "keys/sec", &ins, &sizes, &names); }
    if run_lookup    { print_table("Lookup", "keys/sec", &look, &sizes, &names); }
    if run_fwd       { print_table("Iter forward", "keys/sec", &fwd, &sizes, &names); }
    if run_rev       { print_table("Iter backward", "keys/sec", &rev, &sizes, &names); }
    if run_fwd_idx   { print_table("Iter fwd index", "keys/sec", &fwd_idx, &sizes, &names); }
    if run_rev_idx   { print_table("Iter rev index", "keys/sec", &rev_idx, &sizes, &names); }
    if run_optimize  { print_table("Optimize", "keys/sec", &opt, &sizes, &names); }
    if run_memory    { print_mem_table(&mem, &sizes, &names); }

    // ── Merge and save results ───────────────────────────────────────
    if run_insert   { merge_results(&mut results, "Insertion (keys/sec)",  &ins, &sizes); }
    if run_lookup   { merge_results(&mut results, "Lookup (keys/sec)",     &look, &sizes); }
    if run_fwd      { merge_results(&mut results, "Iter forward (keys/sec)",  &fwd, &sizes); }
    if run_rev      { merge_results(&mut results, "Iter backward (keys/sec)", &rev, &sizes); }
    if run_fwd_idx  { merge_results(&mut results, "Iter fwd index (keys/sec)", &fwd_idx, &sizes); }
    if run_rev_idx  { merge_results(&mut results, "Iter rev index (keys/sec)", &rev_idx, &sizes); }
    if run_optimize { merge_results(&mut results, "Optimize (keys/sec)",  &opt, &sizes); }
    if run_memory   { merge_results(&mut results, "Memory (bytes/key)",   &mem, &sizes); }
    save_results(&results, &json_path, &md_path);

    println!();
}
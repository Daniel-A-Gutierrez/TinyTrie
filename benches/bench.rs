use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::{BTreeMap, HashMap};
use std::hint::black_box;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tiny_trie::{BitTrie, NibbleTrie, PolyTrie, TinyTrie, TinyTrieMap};

// ── Config ──────────────────────────────────────────────────────────

const SIZES: &[usize] = &[10_000, 100_000, 10_000_000];
const BENCH_SECS: u64 = 3;

// ── Contestant registry ────────────────────────────────────────────
// Index 0–6: insert+lookup, 7–8: lookup-only (optimize measured separately).
// HashMap/SortedVec lack reverse iteration; NibbleOpt/PolyOpt are optimized variants.

const CONTESTANT_NAMES: &[&str] = &[
    "TinyTrie", "NibbleTrie", "BitTrie", "PolyTrie",
    "BTreeMap", "HashMap", "SortedVec", "NibbleOpt", "PolyOpt",
];

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

// ── Pre-built structures for seek / iteration ───────────────────────

struct Structures {
    trie: TinyTrie<usize, 6, u8>,
    ntrie: NibbleTrie<usize>,
    ntrie_opt: NibbleTrie<usize>,
    btrie: BitTrie<usize>,
    ptrie: PolyTrie<usize>,
    ptrie_opt: PolyTrie<usize>,
    btree: BTreeMap<Vec<u8>, usize>,
    hmap: HashMap<Vec<u8>, usize>,
    sorted: Vec<(Vec<u8>, usize)>,
    lookup_keys: Vec<Vec<u8>>,
    lookup_keys_null: Vec<Vec<u8>>,
}

fn build_all(keys: &[Vec<u8>]) -> Structures {
    let mut trie = TinyTrie::new();
    let mut ntrie = NibbleTrie::new();
    let mut btrie = BitTrie::new();
    let mut ptrie = PolyTrie::new();
    let mut btree = BTreeMap::new();
    let mut hmap = HashMap::new();
    for (i, k) in keys.iter().enumerate() {
        trie.insert(k.clone(), i).unwrap();
        ntrie.insert(k.clone(), i).unwrap();
        btrie.insert(k.clone(), i).unwrap();
        ptrie.insert(k.clone(), i).unwrap();
        btree.insert(k.clone(), i);
        hmap.insert(k.clone(), i);
    }
    let mut lookup_keys = Vec::with_capacity(keys.len() * 2);
    let mut lookup_keys_null = Vec::with_capacity(keys.len() * 2);
    for k in keys {
        // Hit key (non-null-terminated for std collections)
        lookup_keys.push(k.clone());
        // Hit key (null-terminated for trie lookups)
        let mut nt = k.clone();
        nt.push(0);
        lookup_keys_null.push(nt);
        // Miss key (non-null-terminated)
        let mut miss = k.clone();
        miss.push(b'z');
        lookup_keys.push(miss.clone());
        // Miss key (null-terminated)
        miss.push(0);
        lookup_keys_null.push(miss);
    }
    let mut ptrie_opt = ptrie.clone();
    ptrie_opt.optimize();
    let mut ntrie_opt = ntrie.clone();
    ntrie_opt.optimize();
    Structures { trie, ntrie, ntrie_opt, btrie, ptrie, ptrie_opt, btree, hmap, sorted: build_sorted_vec(keys), lookup_keys, lookup_keys_null }
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

// ── Per-test bench traits ───────────────────────────────────────────

trait InsertBench {
    fn run(keys: &[Vec<u8>]);
}

trait LookupBench {
    fn run(&self, keys_null: &[Vec<u8>], keys: &[Vec<u8>]);
}

trait FwdIterBench {
    fn run(&self);
}

trait RevIterBench {
    fn run(&self);
}

trait OptimizeBench {
    fn run(keys: &[Vec<u8>]);
}

// ── Macro: generate bench trait impls for TinyTrieMap types ────────

macro_rules! impl_trie_benches {
    ($type:ty) => {
        impl InsertBench for $type {
            fn run(keys: &[Vec<u8>]) {
                let mut m = <$type>::trie_new();
                for (i, k) in keys.iter().enumerate() { m.trie_insert(k.clone(), i); }
                black_box(&m);
            }
        }
        impl LookupBench for $type {
            fn run(&self, keys_null: &[Vec<u8>], _keys: &[Vec<u8>]) {
                for k in keys_null { black_box(self.trie_get(k)); }
            }
        }
        impl FwdIterBench for $type {
            fn run(&self) { self.trie_iter_fwd(|k, v| { black_box(k); black_box(v); }); }
        }
        impl RevIterBench for $type {
            fn run(&self) { self.trie_iter_rev(|k, v| { black_box(k); black_box(v); }); }
        }
    };
}

impl_trie_benches!(TinyTrie<usize, 6, u8>);
impl_trie_benches!(BitTrie<usize>);
impl_trie_benches!(PolyTrie<usize>);

// NibbleTrie no longer requires null-terminated keys — use plain keys for lookup.
impl InsertBench for NibbleTrie<usize> {
    fn run(keys: &[Vec<u8>]) {
        let mut m = NibbleTrie::trie_new();
        for (i, k) in keys.iter().enumerate() { m.trie_insert(k.clone(), i); }
        black_box(&m);
    }
}
impl LookupBench for NibbleTrie<usize> {
    fn run(&self, _keys_null: &[Vec<u8>], keys: &[Vec<u8>]) {
        for k in keys { black_box(self.trie_get(k)); }
    }
}
impl FwdIterBench for NibbleTrie<usize> {
    fn run(&self) { self.trie_iter_fwd(|k, v| { black_box(k); black_box(v); }); }
}
impl RevIterBench for NibbleTrie<usize> {
    fn run(&self) { self.trie_iter_rev(|k, v| { black_box(k); black_box(v); }); }
}

// NibbleOpt and PolyOpt use the same types as NibbleTrie/PolyTrie —
// they're just built with optimize(). The trait impls are shared.

// OptimizeBench — only for types that support optimize
impl OptimizeBench for NibbleTrie<usize> {
    fn run(keys: &[Vec<u8>]) {
        let mut m = NibbleTrie::trie_new();
        for (i, k) in keys.iter().enumerate() { m.trie_insert(k.clone(), i); }
        m.trie_optimize();
        black_box(&m);
    }
}

impl OptimizeBench for PolyTrie<usize> {
    fn run(keys: &[Vec<u8>]) {
        let mut m = PolyTrie::trie_new();
        for (i, k) in keys.iter().enumerate() { m.trie_insert(k.clone(), i); }
        m.trie_optimize();
        black_box(&m);
    }
}

// ── Manual trait impls for std collections ──────────────────────────

impl InsertBench for BTreeMap<Vec<u8>, usize> {
    fn run(keys: &[Vec<u8>]) {
        let mut m = BTreeMap::new();
        for (i, k) in keys.iter().enumerate() { m.insert(k.clone(), i); }
        black_box(&m);
    }
}

impl LookupBench for BTreeMap<Vec<u8>, usize> {
    fn run(&self, _keys_null: &[Vec<u8>], keys: &[Vec<u8>]) {
        for k in keys { black_box(self.get(k)); }
    }
}

impl FwdIterBench for BTreeMap<Vec<u8>, usize> {
    fn run(&self) { for (k, v) in self.iter() { black_box(k); black_box(v); } }
}

impl RevIterBench for BTreeMap<Vec<u8>, usize> {
    fn run(&self) { for (k, v) in self.iter().rev() { black_box(k); black_box(v); } }
}

impl InsertBench for HashMap<Vec<u8>, usize> {
    fn run(keys: &[Vec<u8>]) {
        let mut m = HashMap::new();
        for (i, k) in keys.iter().enumerate() { m.insert(k.clone(), i); }
        black_box(&m);
    }
}

impl LookupBench for HashMap<Vec<u8>, usize> {
    fn run(&self, _keys_null: &[Vec<u8>], keys: &[Vec<u8>]) {
        for k in keys { black_box(self.get(k)); }
    }
}

impl InsertBench for Vec<(Vec<u8>, usize)> {
    fn run(keys: &[Vec<u8>]) {
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

impl LookupBench for Vec<(Vec<u8>, usize)> {
    fn run(&self, _keys_null: &[Vec<u8>], keys: &[Vec<u8>]) {
        for k in keys { black_box(sorted_vec_get(self, k)); }
    }
}

impl FwdIterBench for Vec<(Vec<u8>, usize)> {
    fn run(&self) { for (k, v) in self.iter() { black_box(k); black_box(v); } }
}

// ── Contestant ──────────────────────────────────────────────────────

/// Measure memory: snapshot allocator, run build closure (which returns bytes used), compute bytes/key.
/// The build closure must measure allocation before dropping the structure.
fn mem_measure(name: &str, size: usize, mem: &mut ResultMap, build: impl FnOnce() -> u64) {
    let bytes = build();
    eprintln!("    {name}: {bytes} bytes ({:.1}/key)", bytes as f64 / size as f64);
    mem.entry(name.into()).or_default().push(bytes as f64 / size as f64);
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

fn fmt_table(title: &str, unit: &str, data: &ResultMap, fmt_val: fn(f64) -> String) -> String {
    let names: Vec<&str> = CONTESTANT_NAMES.iter()
        .filter(|n| data.contains_key(*n as &str))
        .copied()
        .collect();
    if names.is_empty() { return String::new(); }
    let mut s = format!("\n─── {title} ({unit}) ───\n");
    s.push_str(&format!("{:>12}", ""));
    for &sz in SIZES { s.push_str(&format!("{:>12}", sz)); }
    s.push('\n');
    for name in &names {
        s.push_str(&format!("{:>12}", name));
        for &val in data.get(*name).unwrap() { s.push_str(&format!("{:>12}", fmt_val(val))); }
        s.push('\n');
    }
    s
}

fn print_table(title: &str, unit: &str, data: &ResultMap) {
    let s = fmt_table(title, unit, data, fmt_rate);
    if !s.is_empty() { print!("{s}"); }
}

fn print_mem_table(data: &ResultMap) {
    let s = fmt_table("Memory", "bytes/key", data, fmt_bytes_per);
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

    // Markdown
    let mut md = String::new();
    for (section, rows) in &data.sections {
        let is_mem = section.contains("Memory");
        let fmt: fn(f64) -> String = if is_mem { fmt_bytes_per } else { fmt_rate };
        md.push_str(&format!("\n─── {section} ───\n"));
        md.push_str(&format!("{:>12}", ""));
        for &sz in &data.sizes { md.push_str(&format!("{:>12}", sz)); }
        md.push('\n');
        for (name, vals) in rows {
            md.push_str(&format!("{:>12}", name));
            for &v in vals { md.push_str(&format!("{:>12}", fmt(v))); }
            md.push('\n');
        }
    }
    if !md.is_empty() { md.push('\n'); }
    std::fs::write(RESULTS_MD, &md).unwrap();
    eprintln!("  wrote {RESULTS_MD}");
}

/// Merge new bench results into the persistent data.
/// Only overwrites rows for structures we actually benched in this run.
fn merge_results(data: &mut ResultsFile, section: &str, new: &ResultMap) {
    let sec = data.sections.entry(section.to_string()).or_default();
    for (name, values) in new {
        sec.insert(name.clone(), values.clone());
    }
}

// ── Filter helper ────────────────────────────────────────────────────

fn is_active(name: &str, filter: &Option<String>) -> bool {
    match filter {
        None => true,
        Some(pat) => name.to_ascii_lowercase().contains(pat.as_str()),
    }
}

// ── Main ────────────────────────────────────────────────────────────

fn main() {
    let args: Vec<String> = std::env::args().collect();
    // Skip cargo's own flags (e.g. --bench) — first arg that doesn't start with `--` is the filter
    let filter = args.iter().skip(1)
        .find(|a| !a.starts_with("--"))
        .map(|s| s.to_ascii_lowercase());

    let active: Vec<bool> = CONTESTANT_NAMES.iter().map(|n| is_active(n, &filter)).collect();
    if active.iter().all(|a| !a) {
        eprintln!("No structures match filter {:?}. Available: {}", filter.unwrap(), CONTESTANT_NAMES.join(", "));
        std::process::exit(1);
    }

    let budget = Duration::from_secs(BENCH_SECS);

    println!();
    println!("=== TinyTrie Benchmark Suite ===");
    if let Some(pat) = &filter {
        let names: Vec<&str> = CONTESTANT_NAMES.iter().zip(active.iter()).filter(|(_, a)| **a).map(|(n, _)| *n).collect();
        println!("Filter: {pat} → {}", names.join(", "));
    }
    println!("{BENCH_SECS}s per bench · sequential per size");
    println!();

    let mut ins  = ResultMap::new();
    let mut look = ResultMap::new();
    let mut fwd  = ResultMap::new();
    let mut rev  = ResultMap::new();
    let mut mem  = ResultMap::new();
    let mut opt  = ResultMap::new();

    let mut results = load_results();
    results.sizes = SIZES.to_vec();

    for &size in SIZES {
        eprintln!("[n = {size}]");

        // ── Generate keys ─────────────────────────────────────────────
        eprint!("  generating keys... ");
        let keys = string_keys(size);
        eprintln!("✓");

        // ── Build contestants ──────────────────────────────────────────
        // Insertion runs first (no pre-built structure needed)
        eprintln!("  insertion:");
        let any_insert = active[0] || active[1] || active[2] || active[3] || active[4] || active[5] || active[6];
        if any_insert {
            if active[0] { let r = bench(budget, "TinyTrie", || <TinyTrie<usize, 6, u8> as InsertBench>::run(&keys)); ins.entry("TinyTrie".into()).or_default().push(r.rate(size as u64)); }
            if active[1] { let r = bench(budget, "NibbleTrie", || <NibbleTrie<usize> as InsertBench>::run(&keys)); ins.entry("NibbleTrie".into()).or_default().push(r.rate(size as u64)); }
            if active[2] { let r = bench(budget, "BitTrie", || <BitTrie<usize> as InsertBench>::run(&keys)); ins.entry("BitTrie".into()).or_default().push(r.rate(size as u64)); }
            if active[3] { let r = bench(budget, "PolyTrie", || <PolyTrie<usize> as InsertBench>::run(&keys)); ins.entry("PolyTrie".into()).or_default().push(r.rate(size as u64)); }
            if active[4] { let r = bench(budget, "BTreeMap", || <BTreeMap<Vec<u8>, usize> as InsertBench>::run(&keys)); ins.entry("BTreeMap".into()).or_default().push(r.rate(size as u64)); }
            if active[5] { let r = bench(budget, "HashMap", || <HashMap<Vec<u8>, usize> as InsertBench>::run(&keys)); ins.entry("HashMap".into()).or_default().push(r.rate(size as u64)); }
            if active[6] { let r = bench(budget, "SortedVec", || <Vec<(Vec<u8>, usize)> as InsertBench>::run(&keys)); ins.entry("SortedVec".into()).or_default().push(r.rate(size as u64)); }
        }

        // ── Build structures for lookup / iteration ─────────────────────
        eprint!("  building structures... ");
        let t0 = Instant::now();
        let st = build_all(&keys);
        eprintln!("{:.2}s ✓", t0.elapsed().as_secs_f64());

        let lk = &st.lookup_keys;
        let lk_null = &st.lookup_keys_null;

        // ── Lookup ───────────────────────────────────────────────────
        eprintln!("  lookup:");
        if active[0] { let r = bench(budget, "TinyTrie", || <TinyTrie<usize, 6, u8> as LookupBench>::run(&st.trie, lk_null, lk)); look.entry("TinyTrie".into()).or_default().push(r.rate(lk.len() as u64)); }
        if active[1] { let r = bench(budget, "NibbleTrie", || <NibbleTrie<usize> as LookupBench>::run(&st.ntrie, lk_null, lk)); look.entry("NibbleTrie".into()).or_default().push(r.rate(lk.len() as u64)); }
        if active[2] { let r = bench(budget, "BitTrie", || <BitTrie<usize> as LookupBench>::run(&st.btrie, lk_null, lk)); look.entry("BitTrie".into()).or_default().push(r.rate(lk.len() as u64)); }
        if active[3] { let r = bench(budget, "PolyTrie", || <PolyTrie<usize> as LookupBench>::run(&st.ptrie, lk_null, lk)); look.entry("PolyTrie".into()).or_default().push(r.rate(lk.len() as u64)); }
        if active[4] { let r = bench(budget, "BTreeMap", || <BTreeMap<Vec<u8>, usize> as LookupBench>::run(&st.btree, lk_null, lk)); look.entry("BTreeMap".into()).or_default().push(r.rate(lk.len() as u64)); }
        if active[5] { let r = bench(budget, "HashMap", || <HashMap<Vec<u8>, usize> as LookupBench>::run(&st.hmap, lk_null, lk)); look.entry("HashMap".into()).or_default().push(r.rate(lk.len() as u64)); }
        if active[6] { let r = bench(budget, "SortedVec", || <Vec<(Vec<u8>, usize)> as LookupBench>::run(&st.sorted, lk_null, lk)); look.entry("SortedVec".into()).or_default().push(r.rate(lk.len() as u64)); }
        if active[7] { let r = bench(budget, "NibbleOpt", || <NibbleTrie<usize> as LookupBench>::run(&st.ntrie_opt, lk_null, lk)); look.entry("NibbleOpt".into()).or_default().push(r.rate(lk.len() as u64)); }
        if active[8] { let r = bench(budget, "PolyOpt", || <PolyTrie<usize> as LookupBench>::run(&st.ptrie_opt, lk_null, lk)); look.entry("PolyOpt".into()).or_default().push(r.rate(lk.len() as u64)); }

        // ── Forward iteration ─────────────────────────────────────────
        let any_fwd = active[0] || active[1] || active[2] || active[3] || active[4] || active[6] || active[7] || active[8];
        if any_fwd {
            eprintln!("  iteration (forward):");
            if active[0] { let r = bench(budget, "TinyTrie", || <TinyTrie<usize, 6, u8> as FwdIterBench>::run(&st.trie)); fwd.entry("TinyTrie".into()).or_default().push(r.rate(size as u64)); }
            if active[1] { let r = bench(budget, "NibbleTrie", || <NibbleTrie<usize> as FwdIterBench>::run(&st.ntrie)); fwd.entry("NibbleTrie".into()).or_default().push(r.rate(size as u64)); }
            if active[2] { let r = bench(budget, "BitTrie", || <BitTrie<usize> as FwdIterBench>::run(&st.btrie)); fwd.entry("BitTrie".into()).or_default().push(r.rate(size as u64)); }
            if active[3] { let r = bench(budget, "PolyTrie", || <PolyTrie<usize> as FwdIterBench>::run(&st.ptrie)); fwd.entry("PolyTrie".into()).or_default().push(r.rate(size as u64)); }
            if active[4] { let r = bench(budget, "BTreeMap", || <BTreeMap<Vec<u8>, usize> as FwdIterBench>::run(&st.btree)); fwd.entry("BTreeMap".into()).or_default().push(r.rate(size as u64)); }
            if active[6] { let r = bench(budget, "SortedVec", || <Vec<(Vec<u8>, usize)> as FwdIterBench>::run(&st.sorted)); fwd.entry("SortedVec".into()).or_default().push(r.rate(size as u64)); }
            if active[7] { let r = bench(budget, "NibbleOpt", || <NibbleTrie<usize> as FwdIterBench>::run(&st.ntrie_opt)); fwd.entry("NibbleOpt".into()).or_default().push(r.rate(size as u64)); }
            if active[8] { let r = bench(budget, "PolyOpt", || <PolyTrie<usize> as FwdIterBench>::run(&st.ptrie_opt)); fwd.entry("PolyOpt".into()).or_default().push(r.rate(size as u64)); }
        }

        // ── Backward iteration ───────────────────────────────────────
        let any_rev = active[0] || active[1] || active[2] || active[3] || active[4] || active[7] || active[8];
        if any_rev {
            eprintln!("  iteration (backward):");
            if active[0] { let r = bench(budget, "TinyTrie", || <TinyTrie<usize, 6, u8> as RevIterBench>::run(&st.trie)); rev.entry("TinyTrie".into()).or_default().push(r.rate(size as u64)); }
            if active[1] { let r = bench(budget, "NibbleTrie", || <NibbleTrie<usize> as RevIterBench>::run(&st.ntrie)); rev.entry("NibbleTrie".into()).or_default().push(r.rate(size as u64)); }
            if active[2] { let r = bench(budget, "BitTrie", || <BitTrie<usize> as RevIterBench>::run(&st.btrie)); rev.entry("BitTrie".into()).or_default().push(r.rate(size as u64)); }
            if active[3] { let r = bench(budget, "PolyTrie", || <PolyTrie<usize> as RevIterBench>::run(&st.ptrie)); rev.entry("PolyTrie".into()).or_default().push(r.rate(size as u64)); }
            if active[4] { let r = bench(budget, "BTreeMap", || <BTreeMap<Vec<u8>, usize> as RevIterBench>::run(&st.btree)); rev.entry("BTreeMap".into()).or_default().push(r.rate(size as u64)); }
            if active[7] { let r = bench(budget, "NibbleOpt", || <NibbleTrie<usize> as RevIterBench>::run(&st.ntrie_opt)); rev.entry("NibbleOpt".into()).or_default().push(r.rate(size as u64)); }
            if active[8] { let r = bench(budget, "PolyOpt", || <PolyTrie<usize> as RevIterBench>::run(&st.ptrie_opt)); rev.entry("PolyOpt".into()).or_default().push(r.rate(size as u64)); }
        }

        // ── Optimize time ─────────────────────────────────────────────
        if active[7] || active[8] {
            eprintln!("  optimize:");
            if active[7] { let r = bench(budget, "NibbleOpt", || <NibbleTrie<usize> as OptimizeBench>::run(&keys)); opt.entry("NibbleOpt".into()).or_default().push(r.rate(size as u64)); }
            if active[8] { let r = bench(budget, "PolyOpt", || <PolyTrie<usize> as OptimizeBench>::run(&keys)); opt.entry("PolyOpt".into()).or_default().push(r.rate(size as u64)); }
        }

        // ── Memory (sequential, needs clean allocator state) ────────
        drop(st);
        eprintln!("  memory:");

        if active[0] { mem_measure("TinyTrie", size, &mut mem, || { let before = read_allocated(); let mut m: TinyTrie<usize, 6, u8> = TinyTrie::trie_new(); for (i, k) in keys.iter().enumerate() { m.trie_insert(k.clone(), i); } let bytes = read_allocated() - before; drop(m); bytes }); }
        if active[1] { mem_measure("NibbleTrie", size, &mut mem, || { let before = read_allocated(); let mut m: NibbleTrie<usize> = NibbleTrie::trie_new(); for (i, k) in keys.iter().enumerate() { m.trie_insert(k.clone(), i); } let bytes = read_allocated() - before; drop(m); bytes }); }
        if active[2] { mem_measure("BitTrie", size, &mut mem, || { let before = read_allocated(); let mut m: BitTrie<usize> = BitTrie::trie_new(); for (i, k) in keys.iter().enumerate() { m.trie_insert(k.clone(), i); } let bytes = read_allocated() - before; drop(m); bytes }); }
        if active[3] { mem_measure("PolyTrie", size, &mut mem, || { let before = read_allocated(); let mut m: PolyTrie<usize> = PolyTrie::trie_new(); for (i, k) in keys.iter().enumerate() { m.trie_insert(k.clone(), i); } let bytes = read_allocated() - before; drop(m); bytes }); }
        if active[4] { mem_measure("BTreeMap", size, &mut mem, || { let before = read_allocated(); let mut m: BTreeMap<Vec<u8>, usize> = BTreeMap::new(); for (i, k) in keys.iter().enumerate() { m.insert(k.clone(), i); } let bytes = read_allocated() - before; drop(m); bytes }); }
        if active[5] { mem_measure("HashMap", size, &mut mem, || { let before = read_allocated(); let mut m: HashMap<Vec<u8>, usize> = HashMap::new(); for (i, k) in keys.iter().enumerate() { m.insert(k.clone(), i); } let bytes = read_allocated() - before; drop(m); bytes }); }
        if active[6] { mem_measure("SortedVec", size, &mut mem, || { let before = read_allocated(); let s = build_sorted_vec(&keys); let bytes = read_allocated() - before; drop(s); bytes }); }
        if active[7] { mem_measure("NibbleOpt", size, &mut mem, || { let before = read_allocated(); let mut m: NibbleTrie<usize> = NibbleTrie::trie_new(); for (i, k) in keys.iter().enumerate() { m.trie_insert(k.clone(), i); } m.trie_optimize(); let bytes = read_allocated() - before; drop(m); bytes }); }
        if active[8] { mem_measure("PolyOpt", size, &mut mem, || { let before = read_allocated(); let mut m: PolyTrie<usize> = PolyTrie::trie_new(); for (i, k) in keys.iter().enumerate() { m.trie_insert(k.clone(), i); } m.trie_optimize(); let bytes = read_allocated() - before; drop(m); bytes }); }

        eprintln!();
    }

    // ── Print summary tables ────────────────────────────────────────

    print_table("Insertion", "keys/sec", &ins);
    print_table("Lookup", "keys/sec", &look);
    print_table("Iter forward", "keys/sec", &fwd);
    print_table("Iter backward", "keys/sec", &rev);
    print_table("Optimize", "keys/sec", &opt);
    print_mem_table(&mem);

    // ── Merge and save results ───────────────────────────────────────
    merge_results(&mut results, "Insertion (keys/sec)",  &ins);
    merge_results(&mut results, "Lookup (keys/sec)",     &look);
    merge_results(&mut results, "Iter forward (keys/sec)",  &fwd);
    merge_results(&mut results, "Iter backward (keys/sec)", &rev);
    merge_results(&mut results, "Optimize (keys/sec)",  &opt);
    merge_results(&mut results, "Memory (bytes/key)",   &mem);
    save_results(&results);

    println!();
}
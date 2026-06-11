use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::{BTreeMap, HashMap};
use std::hint::black_box;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tiny_trie::{BitTrie, NibbleTrie, PolyTrie, TinyTrie};

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
    for k in keys {
        lookup_keys.push(k.clone());
        let mut miss = k.clone();
        miss.push(b'z');
        lookup_keys.push(miss);
    }
    let mut ptrie_opt = ptrie.clone();
    ptrie_opt.optimize();
    let mut ntrie_opt = ntrie.clone();
    ntrie_opt.optimize();
    Structures { trie, ntrie, ntrie_opt, btrie, ptrie, ptrie_opt, btree, hmap, sorted: build_sorted_vec(keys), lookup_keys }
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

// ── Macros ──────────────────────────────────────────────────────────

/// Conditionally run a benchmark. Only runs if $cond is true.
/// The body block is wrapped in `||` to form the bench closure.
macro_rules! bench_run {
    ($results:expr, $budget:expr, $rate_arg:expr, $cond:expr, $name:expr, $body:block) => {
        if $cond {
            let r = bench($budget, $name, || $body);
            $results.entry($name.to_string()).or_default().push(r.rate($rate_arg));
        }
    };
}

/// Snapshot allocator, build structure, measure bytes/key, drop.
macro_rules! mem_measure {
    ($active:expr, $name:expr, $size:expr, $results:expr, $build:expr) => {{
        if $active {
            let before = read_allocated();
            let s = $build;
            let bytes = read_allocated() - before;
            drop(s);
            eprintln!("    {}: {} bytes ({:.1}/key)", $name, bytes, bytes as f64 / $size as f64);
            $results.entry($name.to_string()).or_default().push(bytes as f64 / $size as f64);
        }
    }};
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

        // ── Insertion ────────────────────────────────────────────────
        eprintln!("  insertion:");
        bench_run!(ins, budget, size as u64, active[0], "TinyTrie",   { let mut m: TinyTrie<usize, 6, u8> = TinyTrie::new(); for (i, k) in keys.iter().enumerate() { m.insert(k.clone(), i).unwrap(); } black_box(&m); });
        bench_run!(ins, budget, size as u64, active[1], "NibbleTrie", { let mut m: NibbleTrie<usize> = NibbleTrie::new(); for (i, k) in keys.iter().enumerate() { m.insert(k.clone(), i).unwrap(); } black_box(&m); });
        bench_run!(ins, budget, size as u64, active[2], "BitTrie",    { let mut m: BitTrie<usize> = BitTrie::new(); for (i, k) in keys.iter().enumerate() { m.insert(k.clone(), i).unwrap(); } black_box(&m); });
        bench_run!(ins, budget, size as u64, active[3], "PolyTrie",   { let mut m: PolyTrie<usize> = PolyTrie::new(); for (i, k) in keys.iter().enumerate() { m.insert(k.clone(), i).unwrap(); } black_box(&m); });
        bench_run!(ins, budget, size as u64, active[4], "BTreeMap",   { let mut m: BTreeMap<Vec<u8>, usize> = BTreeMap::new(); for (i, k) in keys.iter().enumerate() { m.insert(k.clone(), i); } black_box(&m); });
        bench_run!(ins, budget, size as u64, active[5], "HashMap",    { let mut m: HashMap<Vec<u8>, usize> = HashMap::new(); for (i, k) in keys.iter().enumerate() { m.insert(k.clone(), i); } black_box(&m); });
        bench_run!(ins, budget, size as u64, active[6], "SortedVec",  { let mut v: Vec<(Vec<u8>, usize)> = Vec::new(); for (i, k) in keys.iter().enumerate() { match v.binary_search_by(|e| e.0.as_slice().cmp(k)) { Ok(_) => {} Err(pos) => v.insert(pos, (k.clone(), i)), } } black_box(&v); });

        // ── Build structures for seek / iteration ────────────────────
        eprint!("  building structures... ");
        let t0 = Instant::now();
        let st = build_all(&keys);
        eprintln!("{:.2}s ✓", t0.elapsed().as_secs_f64());

        // ── Lookup ───────────────────────────────────────────────────
        eprintln!("  lookup:");
        let lk = &st.lookup_keys;
        bench_run!(look, budget, lk.len() as u64, active[0], "TinyTrie",   { for k in lk { black_box(st.trie.get(k)); } });
        bench_run!(look, budget, lk.len() as u64, active[1], "NibbleTrie", { for k in lk { let mut nt = k.clone(); nt.push(0); black_box(st.ntrie.get(&nt)); } });
        bench_run!(look, budget, lk.len() as u64, active[2], "BitTrie",    { for k in lk { let mut nt = k.clone(); nt.push(0); black_box(st.btrie.get(&nt)); } });
        bench_run!(look, budget, lk.len() as u64, active[3], "PolyTrie",   { for k in lk { let mut nt = k.clone(); nt.push(0); black_box(st.ptrie.get(&nt)); } });
        bench_run!(look, budget, lk.len() as u64, active[4], "BTreeMap",   { for k in lk { black_box(st.btree.get(k)); } });
        bench_run!(look, budget, lk.len() as u64, active[5], "HashMap",    { for k in lk { black_box(st.hmap.get(k)); } });
        bench_run!(look, budget, lk.len() as u64, active[6], "SortedVec",  { for k in lk { black_box(sorted_vec_get(&st.sorted, k)); } });
        bench_run!(look, budget, lk.len() as u64, active[7], "NibbleOpt",  { for k in lk { let mut nt = k.clone(); nt.push(0); black_box(st.ntrie_opt.get(&nt)); } });
        bench_run!(look, budget, lk.len() as u64, active[8], "PolyOpt",   { for k in lk { let mut nt = k.clone(); nt.push(0); black_box(st.ptrie_opt.get(&nt)); } });

        // ── Forward iteration ─────────────────────────────────────────
        let any_fwd = active[0] || active[1] || active[2] || active[3] || active[4] || active[6] || active[7] || active[8];
        if any_fwd {
            eprintln!("  iteration (forward):");
            bench_run!(fwd, budget, size as u64, active[0], "TinyTrie",   { let mut it = st.trie.iter(); while let Some((k, v)) = it.next() { black_box(k); black_box(v); } });
            bench_run!(fwd, budget, size as u64, active[1], "NibbleTrie", { let mut it = st.ntrie.iter(); while let Some((k, v)) = it.next() { black_box(k); black_box(v); } });
            bench_run!(fwd, budget, size as u64, active[2], "BitTrie",    { let mut it = st.btrie.iter(); while let Some((k, v)) = it.next() { black_box(k); black_box(v); } });
            bench_run!(fwd, budget, size as u64, active[3], "PolyTrie",   { let mut it = st.ptrie.iter(); while let Some((k, v)) = it.next() { black_box(k); black_box(v); } });
            bench_run!(fwd, budget, size as u64, active[4], "BTreeMap",   { for (k, v) in st.btree.iter() { black_box(k); black_box(v); } });
            bench_run!(fwd, budget, size as u64, active[6], "SortedVec",  { for (k, v) in st.sorted.iter() { black_box(k); black_box(v); } });
            bench_run!(fwd, budget, size as u64, active[7], "NibbleOpt",  { let mut it = st.ntrie_opt.iter(); while let Some((k, v)) = it.next() { black_box(k); black_box(v); } });
            bench_run!(fwd, budget, size as u64, active[8], "PolyOpt",   { let mut it = st.ptrie_opt.iter(); while let Some((k, v)) = it.next() { black_box(k); black_box(v); } });
        }

        // ── Backward iteration ───────────────────────────────────────
        let any_rev = active[0] || active[1] || active[2] || active[3] || active[4] || active[7] || active[8];
        if any_rev {
            eprintln!("  iteration (backward):");
            bench_run!(rev, budget, size as u64, active[0], "TinyTrie",   { let mut it = st.trie.iter_last(); while let Some((k, v)) = it.prev() { black_box(k); black_box(v); } });
            bench_run!(rev, budget, size as u64, active[1], "NibbleTrie", { let mut it = st.ntrie.iter_last(); while let Some((k, v)) = it.prev() { black_box(k); black_box(v); } });
            bench_run!(rev, budget, size as u64, active[2], "BitTrie",    { let mut it = st.btrie.iter_last(); while let Some((k, v)) = it.prev() { black_box(k); black_box(v); } });
            bench_run!(rev, budget, size as u64, active[3], "PolyTrie",   { let mut it = st.ptrie.iter_last(); loop { match it.current() { Some((k, v)) => { black_box(k); black_box(v); } None => break, } if it.prev().is_none() { break; } } });
            bench_run!(rev, budget, size as u64, active[4], "BTreeMap",   { for (k, v) in st.btree.iter().rev() { black_box(k); black_box(v); } });
            bench_run!(rev, budget, size as u64, active[7], "NibbleOpt",  { let mut it = st.ntrie_opt.iter_last(); while let Some((k, v)) = it.prev() { black_box(k); black_box(v); } });
            bench_run!(rev, budget, size as u64, active[8], "PolyOpt",   { let mut it = st.ptrie_opt.iter_last(); loop { match it.current() { Some((k, v)) => { black_box(k); black_box(v); } None => break, } if it.prev().is_none() { break; } } });
        }

        // ── Optimize time ─────────────────────────────────────────────
        if active[7] || active[8] {
            eprintln!("  optimize:");
            bench_run!(opt, budget, size as u64, active[7], "NibbleOpt", { let mut m: NibbleTrie<usize> = NibbleTrie::new(); for (i, k) in keys.iter().enumerate() { m.insert(k.clone(), i).unwrap(); } m.optimize(); black_box(&m); });
            bench_run!(opt, budget, size as u64, active[8], "PolyOpt",  { let mut m: PolyTrie<usize> = PolyTrie::new(); for (i, k) in keys.iter().enumerate() { m.insert(k.clone(), i).unwrap(); } m.optimize(); black_box(&m); });
        }

        // ── Memory (sequential, needs clean allocator state) ────────
        drop(st);
        eprintln!("  memory:");

        mem_measure!(active[0], "TinyTrie",   size, mem, { let mut m: TinyTrie<usize, 6, u8> = TinyTrie::new(); for (i, k) in keys.iter().enumerate() { m.insert(k.clone(), i).unwrap(); } m });
        mem_measure!(active[1], "NibbleTrie", size, mem, { let mut m: NibbleTrie<usize> = NibbleTrie::new(); for (i, k) in keys.iter().enumerate() { m.insert(k.clone(), i).unwrap(); } m });
        mem_measure!(active[2], "BitTrie",    size, mem, { let mut m: BitTrie<usize> = BitTrie::new(); for (i, k) in keys.iter().enumerate() { m.insert(k.clone(), i).unwrap(); } m });
        mem_measure!(active[3], "PolyTrie",  size, mem, { let mut m: PolyTrie<usize> = PolyTrie::new(); for (i, k) in keys.iter().enumerate() { m.insert(k.clone(), i).unwrap(); } m });
        mem_measure!(active[4], "BTreeMap",  size, mem, { let mut m: BTreeMap<Vec<u8>, usize> = BTreeMap::new(); for (i, k) in keys.iter().enumerate() { m.insert(k.clone(), i); } m });
        mem_measure!(active[5], "HashMap",   size, mem, { let mut m: HashMap<Vec<u8>, usize> = HashMap::new(); for (i, k) in keys.iter().enumerate() { m.insert(k.clone(), i); } m });
        mem_measure!(active[6], "SortedVec", size, mem, { build_sorted_vec(&keys) });
        mem_measure!(active[7], "NibbleOpt", size, mem, { let mut m: NibbleTrie<usize> = NibbleTrie::new(); for (i, k) in keys.iter().enumerate() { m.insert(k.clone(), i).unwrap(); } m.optimize(); m });
        mem_measure!(active[8], "PolyOpt",  size, mem, { let mut m: PolyTrie<usize> = PolyTrie::new(); for (i, k) in keys.iter().enumerate() { m.insert(k.clone(), i).unwrap(); } m.optimize(); m });

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
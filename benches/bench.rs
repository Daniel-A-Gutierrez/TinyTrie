use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::{BTreeMap, HashMap};
use std::hint::black_box;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tiny_trie::TinyTrie;

// ── Config ──────────────────────────────────────────────────────────

const SIZES: &[usize] = &[10_000, 100_000, 10_000_000];
const BENCH_SECS: u64 = 3;

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
    btree: BTreeMap<Vec<u8>, usize>,
    hmap: HashMap<Vec<u8>, usize>,
    sorted: Vec<(Vec<u8>, usize)>,
    lookup_keys: Vec<Vec<u8>>,
}

fn build_all(keys: &[Vec<u8>]) -> Structures {
    let mut trie = TinyTrie::new();
    let mut btree = BTreeMap::new();
    let mut hmap = HashMap::new();
    for (i, k) in keys.iter().enumerate() {
        trie.insert(k.clone(), i).unwrap();
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
    Structures { trie, btree, hmap, sorted: build_sorted_vec(keys), lookup_keys }
}

// ── Bench harness ───────────────────────────────────────────────────

/// Mutex to serialize progress output from parallel bench threads.
static LOG: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn log(msg: impl std::fmt::Display) {
    let _lock = LOG.lock().unwrap();
    eprintln!("{msg}");
}

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
/// Prints live progress via the serialized logger so parallel threads
/// don't garble each other. Reports: first-iter cost, then every ~1s.
fn bench(budget: Duration, label: &str, f: impl Fn()) -> BenchResult {
    let mut iters = 0u64;
    let start = Instant::now();
    let mut last_report = Duration::ZERO;
    loop {
        f();
        iters += 1;
        let elapsed = start.elapsed();
        if iters == 1 {
            log(format!("    {label}: first iter {:.2}s", elapsed.as_secs_f64()));
            last_report = elapsed;
        } else if elapsed - last_report >= Duration::from_secs(1) {
            log(format!("    {label}: {iters} iters, {:.1}s elapsed", elapsed.as_secs_f64()));
            last_report = elapsed;
        }
        if elapsed >= budget {
            break;
        }
    }
    let elapsed = start.elapsed();
    let per = elapsed.as_secs_f64() / iters as f64;
    if per >= 1.0 {
        log(format!("    {label}: {iters} iters in {:.2}s ({:.2}s/iter) ✓", elapsed.as_secs_f64(), per));
    } else if per >= 0.001 {
        log(format!("    {label}: {iters} iters in {:.2}s ({:.1}ms/iter) ✓", elapsed.as_secs_f64(), per * 1000.0));
    } else {
        log(format!("    {label}: {iters} iters in {:.2}s ({:.1}µs/iter) ✓", elapsed.as_secs_f64(), per * 1e6));
    }
    BenchResult { iters, elapsed }
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
    if bytes >= 1e3 {
        format!("{:.0}", bytes)
    } else {
        format!("{:.1}", bytes)
    }
}

// ── Table printer ───────────────────────────────────────────────────

fn print_table(title: &str, unit: &str, names: &[&str], rows: &[Vec<f64>]) {
    println!();
    println!("─── {title} ({unit}) ───");
    print!("{:>12}", "");
    for &s in SIZES {
        print!("{:>12}", s);
    }
    println!();
    for (name, row) in names.iter().zip(rows) {
        print!("{:>12}", name);
        for &val in row {
            print!("{:>12}", fmt_rate(val));
        }
        println!();
    }
}

// ── Main ────────────────────────────────────────────────────────────

fn main() {
    let budget = Duration::from_secs(BENCH_SECS);

    println!();
    println!("=== TinyTrie Benchmark Suite ===");
    println!("{BENCH_SECS}s per bench · 4 structures run in parallel per size");
    println!();

    // Per-metric result columns, one entry per SIZE
    let mut ins = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
    let mut look = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
    let mut fwd = [Vec::new(), Vec::new(), Vec::new()];
    let mut rev = [Vec::new(), Vec::new()];
    let mut mem = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];

    for &size in SIZES {
        eprintln!("[n = {size}]");

        // ── Generate keys ─────────────────────────────────────────────
        eprint!("  generating keys... ");
        let keys = string_keys(size);
        eprintln!("✓");

        // ── Insertion (parallel) ─────────────────────────────────────
        eprintln!("  insertion:");
        let ir = std::thread::scope(|s| {
            let t = s.spawn(|| {
                bench(budget, "TinyTrie", || {
                    let mut m: TinyTrie<usize, 6, u8> = TinyTrie::new();
                    for (i, k) in keys.iter().enumerate() {
                        m.insert(k.clone(), i).unwrap();
                    }
                    black_box(&m);
                })
            });
            let b = s.spawn(|| {
                bench(budget, "BTreeMap", || {
                    let mut m: BTreeMap<Vec<u8>, usize> = BTreeMap::new();
                    for (i, k) in keys.iter().enumerate() {
                        m.insert(k.clone(), i);
                    }
                    black_box(&m);
                })
            });
            let h = s.spawn(|| {
                bench(budget, "HashMap", || {
                    let mut m: HashMap<Vec<u8>, usize> = HashMap::new();
                    for (i, k) in keys.iter().enumerate() {
                        m.insert(k.clone(), i);
                    }
                    black_box(&m);
                })
            });
            let v = s.spawn(|| {
                bench(budget, "SortedVec", || {
                    let mut v: Vec<(Vec<u8>, usize)> = Vec::new();
                    for (i, k) in keys.iter().enumerate() {
                        match v.binary_search_by(|e| e.0.as_slice().cmp(k)) {
                            Ok(_) => {} // duplicate, skip
                            Err(pos) => v.insert(pos, (k.clone(), i)),
                        }
                    }
                    black_box(&v);
                })
            });
            [t.join().unwrap(), b.join().unwrap(), h.join().unwrap(), v.join().unwrap()]
        });

        for i in 0..4 {
            ins[i].push(ir[i].rate(size as u64));
        }

        // ── Build structures for seek / iteration ────────────────────
        eprint!("  building structures... ");
        let t0 = Instant::now();
        let st = build_all(&keys);
        eprintln!("{:.2}s ✓", t0.elapsed().as_secs_f64());

        // ── Lookup (parallel) ───────────────────────────────────────
        eprintln!("  lookup:");
        let lk = &st.lookup_keys;
        let lr = std::thread::scope(|s| {
            let t = s.spawn(|| bench(budget, "TinyTrie", || { for k in lk { black_box(st.trie.get(k)); } }));
            let b = s.spawn(|| bench(budget, "BTreeMap", || { for k in lk { black_box(st.btree.get(k)); } }));
            let h = s.spawn(|| bench(budget, "HashMap", || { for k in lk { black_box(st.hmap.get(k)); } }));
            let v = s.spawn(|| bench(budget, "SortedVec", || { for k in lk { black_box(sorted_vec_get(&st.sorted, k)); } }));
            [t.join().unwrap(), b.join().unwrap(), h.join().unwrap(), v.join().unwrap()]
        });

        for i in 0..4 {
            look[i].push(lr[i].rate(lk.len() as u64));
        }

        // ── Forward iteration (parallel) ────────────────────────────
        eprintln!("  iteration (forward):");
        let fr = std::thread::scope(|s| {
            let t = s.spawn(|| {
                bench(budget, "TinyTrie", || {
                    let mut it = st.trie.iter();
                    while let Some((k, v)) = it.next() {
                        black_box(k);
                        black_box(v);
                    }
                })
            });
            let b = s.spawn(|| {
                bench(budget, "BTreeMap", || {
                    for (k, v) in st.btree.iter() {
                        black_box(k);
                        black_box(v);
                    }
                })
            });
            let v = s.spawn(|| {
                bench(budget, "SortedVec", || {
                    for (k, v) in st.sorted.iter() {
                        black_box(k);
                        black_box(v);
                    }
                })
            });
            [t.join().unwrap(), b.join().unwrap(), v.join().unwrap()]
        });

        for i in 0..3 {
            fwd[i].push(fr[i].rate(size as u64));
        }

        // ── Backward iteration (parallel) ────────────────────────────
        eprintln!("  iteration (backward):");
        let rr = std::thread::scope(|s| {
            let t = s.spawn(|| {
                bench(budget, "TinyTrie", || {
                    let mut it = st.trie.iter_last();
                    while let Some((k, v)) = it.prev() {
                        black_box(k);
                        black_box(v);
                    }
                })
            });
            let b = s.spawn(|| {
                bench(budget, "BTreeMap", || {
                    for (k, v) in st.btree.iter().rev() {
                        black_box(k);
                        black_box(v);
                    }
                })
            });
            [t.join().unwrap(), b.join().unwrap()]
        });

        rev[0].push(rr[0].rate(size as u64));
        rev[1].push(rr[1].rate(size as u64));

        // ── Memory (sequential, needs clean allocator state) ────────
        drop(st);
        eprintln!("  memory:");
        let before = read_allocated();
        let mut trie: TinyTrie<usize, 6, u8> = TinyTrie::new();
        for (i, k) in keys.iter().enumerate() {
            trie.insert(k.clone(), i).unwrap();
        }
        let trie_bytes = read_allocated() - before;
        drop(trie);
        eprintln!("    TinyTrie: {} bytes ({:.1}/key)", trie_bytes, trie_bytes as f64 / size as f64);

        let before = read_allocated();
        let mut btree: BTreeMap<Vec<u8>, usize> = BTreeMap::new();
        for (i, k) in keys.iter().enumerate() {
            btree.insert(k.clone(), i);
        }
        let btree_bytes = read_allocated() - before;
        drop(btree);
        eprintln!("    BTreeMap: {} bytes ({:.1}/key)", btree_bytes, btree_bytes as f64 / size as f64);

        let before = read_allocated();
        let mut hmap: HashMap<Vec<u8>, usize> = HashMap::new();
        for (i, k) in keys.iter().enumerate() {
            hmap.insert(k.clone(), i);
        }
        let hmap_bytes = read_allocated() - before;
        drop(hmap);
        eprintln!("    HashMap: {} bytes ({:.1}/key)", hmap_bytes, hmap_bytes as f64 / size as f64);

        let before = read_allocated();
        let sorted = build_sorted_vec(&keys);
        let sorted_bytes = read_allocated() - before;
        drop(sorted);
        eprintln!("    SortedVec: {} bytes ({:.1}/key)", sorted_bytes, sorted_bytes as f64 / size as f64);

        mem[0].push(trie_bytes as f64 / size as f64);
        mem[1].push(btree_bytes as f64 / size as f64);
        mem[2].push(hmap_bytes as f64 / size as f64);
        mem[3].push(sorted_bytes as f64 / size as f64);

        eprintln!();
    }

    // ── Print summary tables ────────────────────────────────────────

    print_table(
        "Insertion",
        "keys/sec",
        &["TinyTrie", "BTreeMap", "HashMap", "SortedVec"],
        &ins,
    );
    print_table(
        "Lookup",
        "keys/sec",
        &["TinyTrie", "BTreeMap", "HashMap", "SortedVec"],
        &look,
    );
    print_table(
        "Iter forward",
        "keys/sec",
        &["TinyTrie", "BTreeMap", "SortedVec"],
        &fwd,
    );
    print_table("Iter backward", "keys/sec", &["TinyTrie", "BTreeMap"], &rev);

    // Memory table uses different formatting
    println!();
    println!("─── Memory (bytes/key) ───");
    print!("{:>12}", "");
    for &s in SIZES {
        print!("{:>12}", s);
    }
    println!();
    for (name, row) in ["TinyTrie", "BTreeMap", "HashMap", "SortedVec"].iter().zip(&mem) {
        print!("{:>12}", name);
        for &val in row {
            print!("{:>12}", fmt_bytes_per(val));
        }
        println!();
    }

    println!();
}
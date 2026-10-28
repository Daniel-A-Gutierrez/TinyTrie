//!SkipSet trial bench: the tier index vs the raw BlockSet bsearch vs
//!BTreeSet, same run — insert (base / wrapper / maintained-with-tiers /
//!BTree), lookup (shuffled probes), seqread (parity — separate tier
//!storage must not tax iteration). u64 keys, workload-order builds,
//!cell budget + partial reps (see sorted_insert.rs). SkipSet lookups
//!verified equal to BlockSet's over a probe prefix before timing. size
//!via first CLI arg (default 10k, multiple of 100); workload names
//!among the args (append/random/runs) select a subset; "phases" enables
//!the A/B insert-path ladder, otherwise each workload prints the
//!turn/slide census twice — plain base, then the maintained skipset
//!path (the deltas-consumer view).

use doa::blockset_v1::BlockSet;
use doa::skipset::SkipSet;
use std::collections::BTreeSet;
use std::hint::black_box;
use std::time::{Duration, Instant};

const DEFAULT_N: usize = 10_000;
const CHUNK_LEN: usize = 100;
const RUNS: usize = 10;
///wall clock per structure × workload cell
const RUN_BUDGET: Duration = Duration::from_secs(3);
///probe prefix the tier path is verified against BlockSet over
const VERIFY: usize = 1000;

struct Cell {
    best:    f64,
    avg:     f64,
    partial: Option<usize>,
}

impl Cell {
    fn fmt(&self, name: &str, width: usize) -> String {
        let partial = match self.partial {
            Some(done) => format!(" [partial {done}]"),
            None => String::new(),
        };
        format!("{name} {best:width$.1} (avg {avg:width$.1}){partial}", best = self.best, avg = self.avg)
    }
}

fn reps(deadline: Instant, mut rep: impl FnMut(Instant) -> (usize, bool, f64)) -> Cell {
    let (mut best, mut sum, mut reps, mut partial) = (f64::MAX, 0.0, 0, None);
    while reps < RUNS && Instant::now() < deadline {
        let (done, cut, ns) = rep(deadline);
        best = best.min(ns);
        sum += ns;
        reps += 1;
        if cut {
            partial = Some(done);
            break;
        }
    }
    Cell { best, avg: sum / reps as f64, partial }
}

fn shuffled(n: usize) -> Vec<u64> {
    let mut keys: Vec<u64> = (0..n as u64).collect();
    let mut s = 0x2545_F491_4F6C_DD1Du64;
    for i in (1..n).rev() {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        keys.swap(i, (s % (i as u64 + 1)) as usize);
    }
    keys
}

fn runs(n: usize) -> Vec<u64> {
    assert_eq!(n % CHUNK_LEN, 0);
    let chunks = n / CHUNK_LEN;
    let mut chunks: Vec<Vec<u64>> = (0..chunks)
        .map(|c| ((c * CHUNK_LEN) as u64..((c + 1) * CHUNK_LEN) as u64).collect())
        .collect();
    for c in chunks.iter_mut().step_by(2) {
        c.reverse();
    }
    let mut s = 0x2545_F491_4F6C_DD1Du64;
    for i in (1..chunks.len()).rev() {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        chunks.swap(i, (s % (i as u64 + 1)) as usize);
    }
    chunks.into_iter().flatten().collect()
}

fn check_order(keys: impl Iterator<Item = u64>) {
    let mut prev = None;
    for k in keys {
        if let Some(p) = prev {
            assert!(k > p);
        }
        prev = Some(k);
    }
}

fn insert_block(keys: &[u64]) -> Cell {
    reps(Instant::now() + RUN_BUDGET, |deadline| {
        let mut set = BlockSet::<u64, u32>::new();
        let t = Instant::now();
        let mut done = 0;
        let mut partial = false;
        for &k in keys {
            set.insert(k)
                .unwrap_or_else(|k| panic!("insert failed at {k}"));
            set.deltas().clear();
            done += 1;
            if done & 1023 == 0 && Instant::now() > deadline {
                partial = true;
                break;
            }
        }
        let ns = t.elapsed().as_nanos() as f64 / done as f64;
        assert_eq!(set.len(), done);
        check_order(set.iter().copied());
        black_box(&set);
        (done, partial, ns)
    })
}

///wrapper-overhead control: tiers never build (store stays None → pure
///pass-through to base.insert), so this column is the base path plus the
///is_none branch — only the maintained cell pays tier upkeep
fn insert_skip(keys: &[u64]) -> Cell {
    reps(Instant::now() + RUN_BUDGET, |deadline| {
        let mut set = SkipSet::<u64, u32>::new();
        let t = Instant::now();
        let mut done = 0;
        let mut partial = false;
        for &k in keys {
            set.insert(k)
                .unwrap_or_else(|k| panic!("insert failed at {k}"));
            set.deltas().clear();
            done += 1;
            if done & 1023 == 0 && Instant::now() > deadline {
                partial = true;
                break;
            }
        }
        let ns = t.elapsed().as_nanos() as f64 / done as f64;
        assert_eq!(set.len(), done);
        check_order(set.iter().copied());
        black_box(&set);
        (done, partial, ns)
    })
}

///insert cell with tiers LIVE: first half builds, one rebuild, the second
///half runs incremental upkeep (slides, turns, epoch re-derivations) —
///ns per op over the second half only
fn insert_skip_maintained(keys: &[u64]) -> Cell {
    reps(Instant::now() + RUN_BUDGET, |deadline| {
        let mut set = SkipSet::<u64, u32>::new();
        let half = keys.len() / 2;
        for &k in &keys[..half] {
            set.insert(k).unwrap();
            set.deltas().clear();
        }
        set.rebuild();
        let t = Instant::now();
        let mut done = 0;
        let mut partial = false;
        for &k in &keys[half..] {
            set.insert(k)
                .unwrap_or_else(|k| panic!("insert failed at {k}"));
            set.deltas().clear();
            done += 1;
            if done & 1023 == 0 && Instant::now() > deadline {
                partial = true;
                break;
            }
        }
        let ns = t.elapsed().as_nanos() as f64 / done as f64;
        assert_eq!(set.len(), half + done);
        check_order(set.iter().copied());
        black_box(&set);
        (done, partial, ns)
    })
}

///phase ladder over the maintained insert path: BASE2 = plain base,
///same build structure (second-half timing — the honest control);
///MAINT = base search + full upkeep; DESCENT adds the tier descent
///(discarded); FULL = the real path (descent + window-bounded bsearch)
#[derive(Clone, Copy)]
enum Phase {
    Base2,
    Maint,
    Descent,
    Full,
}

///one timed rep of a phase — returns (ops, cut?, ns). the structure is
///built the same way every time (first half through SkipSet::insert,
///rebuild, then the second half runs the phase under test)
fn one_phased_rep(keys: &[u64], phase: Phase, deadline: Instant) -> (usize, bool, f64) {
    let half = keys.len() / 2;
    let mut done = 0;
    match phase {
        Phase::Base2 => {
            let mut set = BlockSet::<u64, u32>::new();
            for &k in &keys[..half] {
                set.insert(k).unwrap();
                set.deltas().clear();
            }
            let t = Instant::now();
            for &k in &keys[half..] {
                set.insert(k).unwrap();
                set.deltas().clear();
                done += 1;
                if done & 1023 == 0 && Instant::now() > deadline {
                    let ns = t.elapsed().as_nanos() as f64 / done as f64;
                    return (done, true, ns);
                }
            }
            assert_eq!(set.len(), half + done);
            black_box(&set);
            (done, false, t.elapsed().as_nanos() as f64 / done as f64)
        }
        phase => {
            let mut set = SkipSet::<u64, u32>::new();
            for &k in &keys[..half] {
                set.insert(k).unwrap();
                set.deltas().clear();
            }
            set.rebuild();
            let t = Instant::now();
            for &k in &keys[half..] {
                let r = match phase {
                    Phase::Maint => set.insert_maintain_only(k),
                    Phase::Descent => set.insert_descent_only(k),
                    _ => set.insert(k),
                };
                r.unwrap_or_else(|k| panic!("insert failed at {k}"));
                set.deltas().clear();
                done += 1;
                if done & 1023 == 0 && Instant::now() > deadline {
                    let ns = t.elapsed().as_nanos() as f64 / done as f64;
                    return (done, true, ns);
                }
            }
            assert_eq!(set.len(), half + done);
            black_box(&set);
            (done, false, t.elapsed().as_nanos() as f64 / done as f64)
        }
    }
}

///A/B region test: the two phases alternate reps inside one budget —
///each pair shares machine conditions, so per-pair deltas cancel drift
///(chained cells with independent partial cutoffs measured garbage —
///including a negative upkeep). only pairs where BOTH reps completed
///fully count
fn insert_ab(name: &str, keys: &[u64], a: Phase, b: Phase, an: &str, bn: &str) {
    let deadline = Instant::now() + RUN_BUDGET;
    let (mut best_a, mut best_b) = (f64::MAX, f64::MAX);
    let mut deltas: Vec<f64> = Vec::new();
    let mut pairs = 0;
    while pairs < RUNS && Instant::now() < deadline {
        let (_, cut_a, na) = one_phased_rep(keys, a, deadline);
        let (_, cut_b, nb) = one_phased_rep(keys, b, deadline);
        best_a = best_a.min(na);
        best_b = best_b.min(nb);
        if cut_a || cut_b {
            break; //a partial pair's prefix mismatch would poison the delta
        }
        deltas.push(nb - na);
        pairs += 1;
    }
    deltas.sort_by(|x, y| x.total_cmp(y));
    let med = deltas.get(deltas.len() / 2).copied().unwrap_or(f64::NAN);
    let n: usize = keys.len();
    println!(
        "{name:>8} {n:>9}: A/B {an} {best_a:7.1} vs {bn} {best_b:7.1}  Δ(median of {pairs} pairs) {med:+6.1} ns"
    );
}

fn insert_btree(keys: &[u64]) -> Cell {
    reps(Instant::now() + RUN_BUDGET, |deadline| {
        let mut set = BTreeSet::new();
        let t = Instant::now();
        let mut done = 0;
        let mut partial = false;
        for &key in keys {
            assert!(set.insert(key));
            done += 1;
            if done & 1023 == 0 && Instant::now() > deadline {
                partial = true;
                break;
            }
        }
        let ns = t.elapsed().as_nanos() as f64 / done as f64;
        assert_eq!(set.len(), done);
        check_order(set.iter().copied());
        black_box(&set);
        (done, partial, ns)
    })
}

///build both, verify the tier path equals the base path, then probe the
///skip set. fresh tiers (rebuild after the build)
fn lookup_skip(keys: &[u64], probes: &[u64]) -> Cell {
    let mut block = BlockSet::<u64, u32>::new();
    let mut skip = SkipSet::<u64, u32>::new();
    for &k in keys {
        block.insert(k).unwrap();
        skip.insert(k).unwrap();
        block.deltas().clear();
        skip.deltas().clear();
    }
    skip.rebuild();
    for p in probes.iter().take(VERIFY) {
        assert_eq!(
            skip.contains(p),
            block.contains(p),
            "tier path disagrees at {p}"
        );
    }
    timed_probes(probes, |p| {
        black_box(skip.contains(p));
    })
}

fn lookup_block(keys: &[u64], probes: &[u64]) -> Cell {
    let mut set = BlockSet::<u64, u32>::new();
    for &k in keys {
        set.insert(k).unwrap();
        set.deltas().clear();
    }
    timed_probes(probes, |p| {
        black_box(set.contains(p));
    })
}

fn lookup_btree(keys: &[u64], probes: &[u64]) -> Cell {
    let mut sorted = keys.to_vec();
    sorted.sort_unstable();
    let mut set = BTreeSet::new();
    for &key in &sorted {
        set.insert(key);
    }
    timed_probes(probes, |probe| {
        black_box(set.contains(probe));
    })
}

fn timed_probes(probes: &[u64], mut probe: impl FnMut(&u64)) -> Cell {
    reps(Instant::now() + RUN_BUDGET, |deadline| {
        let t = Instant::now();
        let mut done = 0;
        let mut partial = false;
        for p in probes {
            probe(p);
            done += 1;
            if done & 8191 == 0 && Instant::now() > deadline {
                partial = true;
                break;
            }
        }
        (done, partial, t.elapsed().as_nanos() as f64 / done as f64)
    })
}

fn scan_block(keys: &[u64]) -> Cell {
    let mut set = BlockSet::<u64, u32>::new();
    for &k in keys {
        set.insert(k).unwrap();
        set.deltas().clear();
    }
    assert_eq!(set.len(), keys.len());
    timed_scan(keys.len(), |seed| {
        let mut c = 0u64;
        for k in set.iter() {
            c = c.wrapping_add(*k ^ seed);
        }
        c
    })
}

fn scan_skip(keys: &[u64]) -> Cell {
    let mut set = SkipSet::<u64, u32>::new();
    for &k in keys {
        set.insert(k).unwrap();
        set.deltas().clear();
    }
    set.rebuild();
    assert_eq!(set.len(), keys.len());
    timed_scan(keys.len(), |seed| {
        let mut c = 0u64;
        for k in set.iter() {
            c = c.wrapping_add(*k ^ seed);
        }
        c
    })
}

fn scan_btree(keys: &[u64]) -> Cell {
    let mut sorted = keys.to_vec();
    sorted.sort_unstable();
    let mut set = BTreeSet::new();
    for &key in &sorted {
        set.insert(key);
    }
    assert_eq!(set.len(), keys.len());
    timed_scan(keys.len(), |seed| {
        let mut checksum = 0u64;
        for &key in &set {
            checksum = checksum.wrapping_add(key ^ seed);
        }
        checksum
    })
}

fn timed_scan(count: usize, mut pass: impl FnMut(u64) -> u64) -> Cell {
    reps(Instant::now() + RUN_BUDGET, |_deadline| {
        let t = Instant::now();
        let c = pass(0);
        black_box(c);
        (count, false, t.elapsed().as_nanos() as f64 / count as f64)
    })
}

///single-build turn census: turns timed inside grow_rotate (rare, O(cap)
///— clock overhead is free), slide-moved elements counted from the deltas
///streams. decomposes the plain BlockSet insert: turns vs slides vs the
///rest (bsearch, banking, ladder)
fn turn_census(name: &str, keys: &[u64]) {
    use doa::blockset_v1::{TURN_COUNT, TURN_MOVED, TURN_TIME_NS};
    use std::sync::atomic::Ordering;
    TURN_COUNT.store(0, Ordering::Relaxed);
    TURN_MOVED.store(0, Ordering::Relaxed);
    TURN_TIME_NS.store(0, Ordering::Relaxed);
    let mut set = BlockSet::<u64, u32>::new();
    let mut moved_elements = 0u64;
    let mut plain_inserts = 0u64;
    let mut plain_moved = 0u64;
    let mut gather_inserts = 0u64;
    let mut gather_moved = 0u64;
    let start = Instant::now();
    for &key in keys {
        set.insert(key).unwrap();
        let moved = set.deltas().iter().filter(|(from, to)| from != to).count() as u64;
        moved_elements += moved;
        //small vs large movement — a size-class split, not FREE_STRIDE:
        //word-scoped near/borrow slides legitimately reach ~62 elements
        if moved >= 8 {
            gather_inserts += 1;
            gather_moved += moved;
        } else {
            plain_inserts += 1;
            plain_moved += moved;
        }
    }
    let build_ns = start.elapsed().as_nanos() as u64;
    census_print(
        name,
        &set,
        build_ns,
        keys.len() as u64,
        moved_elements,
        plain_inserts,
        plain_moved,
        gather_inserts,
        gather_moved,
    );
}

///census tail shared by both paths: desert profile + build/turn/slide
///decomposition. the turn statics are caller-scoped — the blockset
///census reads them over its whole build, the maintained census resets
///them after rebuild so only the timed half counts
#[allow(clippy::too_many_arguments)]
fn census_print(
    name: &str,
    set: &BlockSet<u64, u32>,
    build_ns: u64,
    count: u64,
    moved_elements: u64,
    plain_inserts: u64,
    plain_moved: u64,
    gather_inserts: u64,
    gather_moved: u64,
) {
    use doa::blockset_v1::{TURN_COUNT, TURN_MOVED, TURN_TIME_NS};
    use std::sync::atomic::Ordering;
    let turns = TURN_COUNT.load(Ordering::Relaxed);
    let turn_moved = TURN_MOVED.load(Ordering::Relaxed);
    let turn_ns = TURN_TIME_NS.load(Ordering::Relaxed);
    let count = count as f64;
    //desert census: solid runs the banking left behind
    let mut runs = set.live_run_lengths();
    runs.sort_unstable();
    let desert: Vec<usize> = runs.iter().copied().filter(|&run| run > 8).collect();
    let desert_coverage: usize = desert.iter().sum();
    let len = set.len();
    let cap = set.cap();
    println!(
        "          deserts(>8): {} runs, max {}, mean {:.1}, covering {:.1}% of live | len {len} cap {cap} ({:.0}% dense)",
        desert.len(),
        runs.last().copied().unwrap_or(0),
        desert.iter().sum::<usize>() as f64 / desert.len().max(1) as f64,
        desert_coverage as f64 / len as f64 * 100.0,
        len as f64 / cap as f64 * 100.0,
    );
    println!(
        "{name:>8}: build {:6.1} ns/insert | turns {} (relocating {} slots, {:5.1} ns/insert = {:4.1}%) | slides moved {} elems ({:.2}/insert) | plain {}×{:.1} gather {}×{:.1}",
        build_ns as f64 / count,
        turns,
        turn_moved,
        turn_ns as f64 / count,
        turn_ns as f64 / build_ns as f64 * 100.0,
        moved_elements,
        moved_elements as f64 / count,
        plain_inserts,
        plain_moved as f64 / plain_inserts.max(1) as f64,
        gather_inserts,
        gather_moved as f64 / gather_inserts.max(1) as f64,
    );
}

///maintained-path census: the same decomposition through SkipSet with
///tiers live — half build, rebuild, then the timed half runs full
///inserts (upkeep included), so the deltas stats are exactly what the
///tier upkeep had to absorb
fn skip_census(name: &str, keys: &[u64]) {
    use doa::blockset_v1::{TURN_COUNT, TURN_MOVED, TURN_TIME_NS};
    use std::sync::atomic::Ordering;
    let half = keys.len() / 2;
    let mut set = SkipSet::<u64, u32>::new();
    for &k in &keys[..half] {
        set.insert(k).unwrap();
        set.deltas().clear();
    }
    set.rebuild();
    TURN_COUNT.store(0, Ordering::Relaxed);
    TURN_MOVED.store(0, Ordering::Relaxed);
    TURN_TIME_NS.store(0, Ordering::Relaxed);
    let start = Instant::now();
    let (mut moved_elements, mut plain_inserts, mut plain_moved, mut gather_inserts, mut gather_moved) =
        (0u64, 0u64, 0u64, 0u64, 0u64);
    for &k in &keys[half..] {
        set.insert(k).unwrap();
        //same ≥8 split as turn_census: plain slides move < FREE_STRIDE (8)
        let moved = set.deltas().iter().filter(|(from, to)| from != to).count() as u64;
        moved_elements += moved;
        if moved >= 8 {
            gather_inserts += 1;
            gather_moved += moved;
        } else {
            plain_inserts += 1;
            plain_moved += moved;
        }
    }
    let build_ns = start.elapsed().as_nanos() as u64;
    assert_eq!(set.len(), keys.len());
    check_order(set.iter().copied());
    black_box(&set);
    census_print(
        name,
        set.base(),
        build_ns,
        (keys.len() - half) as u64,
        moved_elements,
        plain_inserts,
        plain_moved,
        gather_inserts,
        gather_moved,
    );
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let n: usize = args.first().map(|a| a.parse().expect("size arg")).unwrap_or(DEFAULT_N);
    let probes = shuffled(n);
    //named workload args select a subset; no names = all three
    const NAMES: [&str; 3] = ["append", "random", "runs"];
    let picked: Vec<&str> = args.iter().filter_map(|a| NAMES.iter().find(|w| a == *w)).copied().collect();
    let workloads: Vec<(&str, Vec<u64>)> = [
        ("append", (0..n as u64).collect()),
        ("random", shuffled(n)),
        ("runs", runs(n)),
    ]
    .into_iter()
    .filter(|(w, _)| picked.is_empty() || picked.contains(w))
    .collect();
    //named flag "phases" enables the A/B phase ladder (cargo injects its
    //own positional args, so position-based gating misfires)
    let phases = args.iter().any(|arg| arg == "phases");
    for (name, keys) in &workloads {
        let cells = [
            insert_block(keys),
            insert_skip(keys),
            insert_btree(keys),
            insert_skip_maintained(keys),
            lookup_block(keys, &probes),
            lookup_skip(keys, &probes),
            lookup_btree(keys, &probes),
            scan_block(keys),
            scan_skip(keys),
            scan_btree(keys),
        ];
        println!(
            "{name:>8} {n:>9}: insert {} {} BTree {} | maint {} ns",
            cells[0].fmt("BlockSet", 8),
            cells[1].fmt("SkipSet", 8),
            cells[2].fmt("", 8),
            cells[3].fmt("", 8)
        );
        println!(
            "{name:>8} {n:>9}: lookup {} SkipSet {} BTree {} ns",
            cells[4].fmt("BlockSet", 7),
            cells[5].fmt("", 7),
            cells[6].fmt("", 7)
        );
        println!(
            "{name:>8} {n:>9}: seqread {} {} BTree {} ns/elem",
            cells[7].fmt("BlockSet", 6),
            cells[8].fmt("SkipSet", 6),
            cells[9].fmt("", 6)
        );
        if !phases {
            turn_census(name, keys);
            skip_census(name, keys);
            continue;
        }
        //A/B region tests, one at a time: each pair's median delta
        //attributes one region of the insert path
        insert_ab(name, keys, Phase::Base2, Phase::Maint, "base2", "+upkeep");
        insert_ab(name, keys, Phase::Maint, Phase::Descent, "+upkeep", "+descent");
        insert_ab(name, keys, Phase::Descent, Phase::Full, "+descent", "full");
    }
}
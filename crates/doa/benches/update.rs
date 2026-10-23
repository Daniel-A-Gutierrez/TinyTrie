//!SkipSet::update bench: the addr-hinted replace vs BTreeSet's
//!remove+insert idiom. u64 keys, even-key ascending prefill (pure
//!push_backs — no slide events, the bench's addr vec stays valid
//!untouched), tiers built once, then the timed pass. near: key 2i →
//!2i+1 — old's exact bracket, the in-place fast path (no events, names
//!stable, asserted per op). far: shuffled slots → shuffled distinct odd
//!keys — removals and landings spread uniformly so the ladder amortizes
//!(the ADVERSARIAL cross pattern lives in `diag`: an insert edge
//!marching away from a one-sided desert slides a linearly growing run —
//!the base ladder's known pathology, not update's). each far op's
//!deltas are APPLIED to the bench's addr vec via a HashMap retarget
//!(the holder-protocol cost, included in the timing; the dead name
//!drops from the index BEFORE applying — a mover can re-adopt it).
//!cell budget + partial reps (see sorted_insert.rs). size via first CLI
//!arg, default 10k.

use doa::blockset::{TURN_COUNT, TURN_MOVED, TURN_TIME_NS};
use doa::skipset::SkipSet;
use std::collections::{BTreeSet, HashMap};
use std::hint::black_box;
use std::sync::atomic::Ordering::Relaxed;
use std::time::{Duration, Instant};

const DEFAULT_N: usize = 10_000;
const RUNS: usize = 10;
///wall clock per structure × workload cell
const RUN_BUDGET: Duration = Duration::from_secs(3);

struct Cell {
    best:    f64,
    avg:     f64,
    partial: Option<usize>,
}

impl Cell {
    fn fmt(&self, name: &str) -> String {
        let partial = match self.partial {
            Some(done) => format!(" [partial {done}]"),
            None => String::new(),
        };
        format!("{name} {best:.1} (avg {avg:.1}){partial}", best = self.best, avg = self.avg)
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

fn check_order(keys: impl Iterator<Item = u64>) {
    let mut prev = None;
    for k in keys {
        if let Some(p) = prev {
            assert!(k > p);
        }
        prev = Some(k);
    }
}

///even-key ascending prefill + one rebuild — the addr vec needs no
///retargeting (appends record no events)
fn prefill(n: usize) -> (SkipSet<u64, u32>, Vec<u32>) {
    let mut set = SkipSet::new();
    let mut addrs = Vec::with_capacity(n);
    for i in 0..n as u64 {
        let a = set.insert(2 * i).unwrap();
        set.deltas().clear();
        addrs.push(a);
    }
    set.rebuild();
    (set, addrs)
}

fn update_near(n: usize) -> Cell {
    reps(Instant::now() + RUN_BUDGET, |deadline| {
        let (mut set, addrs) = prefill(n);
        let t = Instant::now();
        let mut done = 0;
        let mut partial = false;
        for (i, &a) in addrs.iter().enumerate() {
            let (a2, old) = set.update(a, 2 * i as u64 + 1).unwrap_or_else(|_| panic!("near {i}"));
            //the bracket holds by construction — the fast path must keep
            //the name and return the even key
            assert_eq!((a2, old), (a, 2 * i as u64), "near op {i}");
            done += 1;
            if done & 1023 == 0 && Instant::now() > deadline {
                partial = true;
                break;
            }
        }
        let ns = t.elapsed().as_nanos() as f64 / done as f64;
        if !partial {
            assert_eq!(set.len(), n);
            assert!(set.contains(&1) && set.contains(&(2 * n as u64 - 1)));
            assert!(!set.contains(&0));
            check_order(set.iter().copied());
        }
        black_box(&set);
        (done, partial, ns)
    })
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

///realistic slow path: shuffled slots, each updated to a distinct shuffled
///odd key — removals and landings spread uniformly, so the ladder's
///space finds amortize. each op's deltas are APPLIED to the addr vec via
///a HashMap retarget (the holder-protocol cost, included in the timing;
///the dead name drops from the index BEFORE applying — a mover can
///re-adopt it)
fn update_far(n: usize) -> Cell {
    reps(Instant::now() + RUN_BUDGET, |deadline| {
        let (mut set, mut addrs) = prefill(n);
        let order = shuffled(n);
        let targets: Vec<u64> = shuffled(n).iter().map(|&i| 2 * i + 1).collect();
        let mut index: HashMap<u32, usize> =
            addrs.iter().enumerate().map(|(i, &a)| (a, i)).collect();
        let t = Instant::now();
        let mut done = 0;
        let mut partial = false;
        for step in 0..n {
            let j = order[step] as usize;
            let a = addrs[j];
            index.remove(&a);
            let (a2, _old) = set.update(a, targets[step]).unwrap_or_else(|_| panic!("far {step}"));
            for (from, to) in set.deltas().iter() {
                let k = index.remove(&from).unwrap();
                index.insert(to, k);
                addrs[k] = to;
            }
            index.insert(a2, j);
            addrs[j] = a2;
            done += 1;
            if done & 1023 == 0 && Instant::now() > deadline {
                partial = true;
                break;
            }
        }
        let ns = t.elapsed().as_nanos() as f64 / done as f64;
        if !partial {
            assert_eq!(set.len(), n);
            assert!(!set.contains(&0));
            check_order(set.iter().copied());
            //every retargeted addr resolves — the holder protocol held
            for &a in &addrs {
                assert!(set.get(a).is_some(), "holder addr {a} dead after {done} ops");
            }
        }
        black_box(&addrs);
        black_box(&set);
        (done, partial, ns)
    })
}

fn btree_near(n: usize) -> Cell {
    reps(Instant::now() + RUN_BUDGET, |deadline| {
        let mut set: BTreeSet<u64> = (0..n as u64).map(|i| 2 * i).collect();
        let t = Instant::now();
        let mut done = 0;
        let mut partial = false;
        for i in 0..n {
            assert!(set.remove(&(2 * i as u64)));
            set.insert(2 * i as u64 + 1);
            done += 1;
            if done & 1023 == 0 && Instant::now() > deadline {
                partial = true;
                break;
            }
        }
        let ns = t.elapsed().as_nanos() as f64 / done as f64;
        black_box(&set);
        (done, partial, ns)
    })
}

fn btree_far(n: usize) -> Cell {
    reps(Instant::now() + RUN_BUDGET, |deadline| {
        let mut set: BTreeSet<u64> = (0..n as u64).map(|i| 2 * i).collect();
        let order = shuffled(n);
        let targets: Vec<u64> = shuffled(n).iter().map(|&i| 2 * i + 1).collect();
        let t = Instant::now();
        let mut done = 0;
        let mut partial = false;
        for step in 0..n {
            let j = order[step] as usize;
            assert!(set.remove(&(2 * j as u64)));
            set.insert(targets[step]);
            done += 1;
            if done & 1023 == 0 && Instant::now() > deadline {
                partial = true;
                break;
            }
        }
        let ns = t.elapsed().as_nanos() as f64 / done as f64;
        black_box(&set);
        (done, partial, ns)
    })
}

///the ADVERSARIAL cross pattern, un-timed diagnostics (turn census +
///event/pair counts): slot i → key 2·((i+n/2) mod n)+1 — bottom-half
///removals, top-half inserts, then mirrored. the pairs/op series
///exposes the base ladder's one-sided-desert pathology: the second
///half inserts at an edge marching AWAY from every remaining hole, so
///each op slides a run one longer (max n/2 moved at the last op). the
///documented multi-word-banking desert concern (CLAUDE.md Future Work),
///NOT update machinery. run with the "diag" arg
fn diag(n: usize) {
    let (mut set, mut addrs) = prefill(n);
    let mut index: HashMap<u32, usize> = addrs.iter().enumerate().map(|(i, &a)| (a, i)).collect();
    let (t0, m0, ns0) = (
        TURN_COUNT.load(Relaxed),
        TURN_MOVED.load(Relaxed),
        TURN_TIME_NS.load(Relaxed),
    );
    let t = Instant::now();
    let (mut pairs, mut events) = (0, 0);
    let mut per_op = Vec::new();
    for i in 0..n {
        let new = 2 * ((i + n / 2) % n) as u64 + 1;
        let a = addrs[i];
        index.remove(&a);
        let (a2, _old) = set.update(a, new).unwrap();
        if set.deltas().is_pending() {
            events += 1;
        }
        let mut moved = 0;
        for (from, to) in set.deltas().iter() {
            let j = index.remove(&from).unwrap();
            index.insert(to, j);
            addrs[j] = to;
            moved += 1;
        }
        if moved > 0 {
            per_op.push((i, moved));
        }
        pairs += moved;
        index.insert(a2, i);
        addrs[i] = a2;
    }
    let moving = per_op.len();
    println!("ops moving elements: {moving}/{n}, max {} (op {})", {
        per_op.iter().map(|(_, m)| *m).max().unwrap_or(0)
    }, {
        per_op.iter().max_by_key(|(_, m)| *m).map(|(i, _)| *i).unwrap_or(0)
    });
    let head: Vec<String> = per_op.iter().take(10).map(|(i, m)| format!("{i}:{m}")).collect();
    println!("first moving ops: {}", head.join(" "));
    println!(
        "far diag: {} ns/op | turns {} ({} ns total, {} moved) | events {}/{} | pairs {}",
        t.elapsed().as_nanos() as f64 / n as f64,
        TURN_COUNT.load(Relaxed) - t0,
        TURN_TIME_NS.load(Relaxed) - ns0,
        TURN_MOVED.load(Relaxed) - m0,
        events,
        n,
        pairs,
    );
}

fn main() {
    let n: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(DEFAULT_N);
    if std::env::args().any(|a| a == "diag") {
        diag(n);
        return;
    }
    println!("n = {n} (even-key prefill, tiers built)");
    println!("{}", update_near(n).fmt("skipset near (fast path) "));
    println!("{}", btree_near(n).fmt("btree   near               "));
    println!("{}", update_far(n).fmt("skipset far  (slow path) "));
    println!("{}", btree_far(n).fmt("btree   far                "));
}
//!`BlockBiMap` vs a bidirectional control — two std BTreeMaps (`K→V` +
//!`V→K`) — by workload: append/prepend/random/runs (see sorted_insert.rs).
//!32-byte Item keys, u64 values. cells: insert (both sides), lookup by
//!key, lookup by value, remove (both sides, shuffled removal order),
//!iter_pairs (full first→last (K, V) pass, seed-mixed checksum). size via
//!first CLI arg (default 10k; multiple of 100). each cell capped at
//!RUN_BUDGET (best/avg over the reps that fit; partial reps marked).
//!bimap's deltas are applied-and-cleared inline — holder churn unmeasured.

use doa::blockbimap::BlockBiMap;

///drain both event slots — this bench holds no name mirrors
fn drain(map: &mut BlockBiMap<Item, u64, u32>) {
    let (keys, values) = map.deltas();
    keys.clear();
    values.clear();
}
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::hint::black_box;
use std::time::{Duration, Instant};

const DEFAULT_N: usize = 10_000;
const CHUNK_LEN: usize = 100;
const RUNS: usize = 10;
///wall clock per structure × workload cell
const RUN_BUDGET: Duration = Duration::from_secs(3);

///32-byte key — orders by the embedded u64
#[derive(Clone, Copy, Debug)]
struct Item {
    key: u64,
    _pad: [u8; 24],
}

impl Item {
    fn new(key: u64) -> Self {
        Item { key, _pad: [0; 24] }
    }
    fn val(key: u64) -> u64 {
        key * 7 + 3
    }
}

impl PartialEq for Item {
    fn eq(&self, o: &Self) -> bool {
        self.key == o.key
    }
}
impl Eq for Item {}
impl PartialOrd for Item {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Item {
    fn cmp(&self, o: &Self) -> Ordering {
        self.key.cmp(&o.key)
    }
}

///best/avg ns per op over the reps that fit the cell budget. `partial` =
///Some(ops done) when the last rep was cut at the deadline — the figures
///are over that prefix
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

///rep driver: reps until RUNS or the deadline; `rep` returns (ops done,
///cut?, elapsed ns) — a cut rep marks the cell partial and ends the run
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

///chunks of 100 sequential keys; even chunks reversed to descending runs;
///chunk order xorshift-shuffled
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

///workload-order build — the block's layout is the workload's
fn build_bimap(keys: &[u64]) -> BlockBiMap<Item, u64, u32> {
    let mut map = BlockBiMap::new();
    for &k in keys {
        map.insert(Item::new(k), Item::val(k)).unwrap();
        drain(&mut map);
    }
    assert_eq!(map.len(), keys.len());
    check_order(map.iter().map(|(i, _)| i.key));
    check_order(map.iter_by_value().map(|(v, _)| *v));
    map
}

fn build_ctrl(keys: &[u64]) -> (BTreeMap<Item, u64>, BTreeMap<u64, Item>) {
    let (mut m1, mut m2) = (BTreeMap::new(), BTreeMap::new());
    for &k in keys {
        let item = Item::new(k);
        let v = Item::val(k);
        assert!(m1.insert(item, v).is_none(), "dup key {k}");
        assert!(m2.insert(v, item).is_none(), "dup val {v}");
    }
    (m1, m2)
}

///insert all pairs (or the prefix the budget allows), ns per pair (both
///sides)
fn insert_bimap(keys: &[u64]) -> Cell {
    reps(Instant::now() + RUN_BUDGET, |deadline| {
        let mut map = BlockBiMap::new();
        let t = Instant::now();
        let mut done = 0;
        let mut partial = false;
        for &k in keys {
            map.insert(Item::new(k), Item::val(k))
                .unwrap_or_else(|(i, _)| panic!("insert failed at {}", i.key));
            drain(&mut map);
            done += 1;
            if done & 1023 == 0 && Instant::now() > deadline {
                partial = true;
                break;
            }
        }
        let ns = t.elapsed().as_nanos() as f64 / done as f64;
        assert_eq!(map.len(), done);
        check_order(map.iter().map(|(i, _)| i.key));
        black_box(&map);
        (done, partial, ns)
    })
}

fn insert_ctrl(keys: &[u64]) -> Cell {
    reps(Instant::now() + RUN_BUDGET, |deadline| {
        let (mut m1, mut m2) = (BTreeMap::new(), BTreeMap::new());
        let t = Instant::now();
        let mut done = 0;
        let mut partial = false;
        for &k in keys {
            let item = Item::new(k);
            let v = Item::val(k);
            assert!(m1.insert(item, v).is_none());
            assert!(m2.insert(v, item).is_none());
            done += 1;
            if done & 1023 == 0 && Instant::now() > deadline {
                partial = true;
                break;
            }
        }
        let ns = t.elapsed().as_nanos() as f64 / done as f64;
        assert_eq!(m1.len(), done);
        assert_eq!(m2.len(), done);
        black_box((&m1, &m2));
        (done, partial, ns)
    })
}

///probe every key / every value (shuffled probe order), ns per lookup
fn lookup_bimap(map: &BlockBiMap<Item, u64, u32>, by_key: bool, probes: &[u64]) -> Cell {
    if by_key {
        timed_probes(probes, |k| {
            black_box(map.get_by_key(&Item::new(*k)));
        })
    } else {
        timed_probes(probes, |v| {
            black_box(map.get_by_value(v));
        })
    }
}

fn lookup_ctrl(m1: &BTreeMap<Item, u64>, m2: &BTreeMap<u64, Item>, by_key: bool, probes: &[u64]) -> Cell {
    if by_key {
        timed_probes(probes, |k| {
            black_box(m1.get(&Item::new(*k)));
        })
    } else {
        timed_probes(probes, |v| {
            black_box(m2.get(v));
        })
    }
}

fn timed_probes(probes: &[u64], mut probe: impl FnMut(&u64)) -> Cell {
    reps(Instant::now() + RUN_BUDGET, |deadline| {
        let t = Instant::now();
        let mut done = 0;
        let mut partial = false;
        for k in probes {
            probe(k);
            done += 1;
            if done & 8191 == 0 && Instant::now() > deadline {
                partial = true;
                break;
            }
        }
        (done, partial, t.elapsed().as_nanos() as f64 / done as f64)
    })
}

///remove every pair by key (shuffled removal order), ns per pair (both
///sides); the build is outside timing
fn remove_bimap(keys: &[u64], order: &[u64]) -> Cell {
    reps(Instant::now() + RUN_BUDGET, |deadline| {
        let mut map = build_bimap(keys);
        let t = Instant::now();
        let mut done = 0;
        let mut partial = false;
        for &k in order {
            map.remove_by_key(&Item::new(k))
                .unwrap_or_else(|| panic!("remove failed at {k}"));
            done += 1;
            if done & 1023 == 0 && Instant::now() > deadline {
                partial = true;
                break;
            }
        }
        (done, partial, t.elapsed().as_nanos() as f64 / done as f64)
    })
}

fn remove_ctrl(keys: &[u64], order: &[u64]) -> Cell {
    reps(Instant::now() + RUN_BUDGET, |deadline| {
        let (mut m1, mut m2) = build_ctrl(keys);
        let t = Instant::now();
        let mut done = 0;
        let mut partial = false;
        for &k in order {
            let v = m1.remove(&Item::new(k)).unwrap_or_else(|| panic!("remove failed at {k}"));
            assert!(m2.remove(&v).is_some());
            done += 1;
            if done & 1023 == 0 && Instant::now() > deadline {
                partial = true;
                break;
            }
        }
        (done, partial, t.elapsed().as_nanos() as f64 / done as f64)
    })
}

///seq read: full first→last (K, V) pass, ns per element (order gated once,
///outside timing)
fn scan_bimap(keys: &[u64]) -> Cell {
    let map = build_bimap(keys);
    timed_scan(keys.len(), |seed| {
        let mut c = 0u64;
        for (i, v) in map.iter() {
            c = c.wrapping_add(i.key ^ seed).wrapping_add(*v ^ seed);
        }
        c
    })
}

fn scan_ctrl(keys: &[u64]) -> Cell {
    let (m1, _) = build_ctrl(keys);
    timed_scan(keys.len(), |seed| {
        let mut c = 0u64;
        for (i, v) in &m1 {
            c = c.wrapping_add(i.key ^ seed).wrapping_add(*v ^ seed);
        }
        c
    })
}

///one full pass per rep returning a checksum; the seed mixes into each
///element so the pass can't be hoisted out of the timing loop
fn timed_scan(count: usize, mut pass: impl FnMut(u64) -> u64) -> Cell {
    reps(Instant::now() + RUN_BUDGET, |_deadline| {
        let t = Instant::now();
        let c = pass(0);
        black_box(c);
        (count, false, t.elapsed().as_nanos() as f64 / count as f64)
    })
}

fn main() {
    let n: usize = std::env::args()
        .nth(1)
        .map(|a| a.parse().expect("size arg"))
        .unwrap_or(DEFAULT_N);
    let keys_probe = shuffled(n);
    let vals_probe: Vec<u64> = keys_probe.iter().map(|k| Item::val(*k)).collect();
    let rm_order = shuffled(n);
    let workloads: [(&str, Vec<u64>); 4] = [
        ("append", (0..n as u64).collect()),
        ("prepend", (0..n as u64).rev().collect()),
        ("random", shuffled(n)),
        ("runs", runs(n)),
    ];
    for (name, keys) in &workloads {
        let insert = [insert_bimap(keys), insert_ctrl(keys)];
        let bimap = build_bimap(keys);
        let (m1, m2) = build_ctrl(keys);
        let lk = [
            lookup_bimap(&bimap, true, &keys_probe),
            lookup_ctrl(&m1, &m2, true, &keys_probe),
        ];
        let lv = [
            lookup_bimap(&bimap, false, &vals_probe),
            lookup_ctrl(&m1, &m2, false, &vals_probe),
        ];
        let rm = [remove_bimap(keys, &rm_order), remove_ctrl(keys, &rm_order)];
        let scan = [scan_bimap(keys), scan_ctrl(keys)];
        for (op, cells) in [
            ("insert", &insert),
            ("look_key", &lk),
            ("look_val", &lv),
            ("remove", &rm),
            ("iter_pairs", &scan),
        ] {
            println!(
                "{name:>8} {n:>9}: {op} {} {} ns",
                cells[0].fmt("BlockBiMap", 9),
                cells[1].fmt("2xBTreeMap", 9)
            );
        }
    }
}
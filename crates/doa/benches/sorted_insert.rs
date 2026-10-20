//!1:1 vs `sorted-vec` crate and `std::collections::BTreeSet` by workload:
//!sequential append (ascending keys), sequential prepend (descending),
//!random (xorshift-shuffled), runs (chunks of 100 sequential keys, half
//!reversed to descending runs, chunk order shuffled) — 32-byte items
//!(index-sized payload — element moves pay realistic byte costs), insert +
//!lookup + seq-read (iter first→last, key checksum). size via first CLI
//!arg (default 10k; must be a multiple of 100 — runs chunks are 100 long).
//!each structure × workload cell is capped at RUN_BUDGET: fast cells
//!complete all RUNS reps, a slow one finishes a single partial rep
//!reported over its completed prefix (marked `[partial done]`). len +
//!strict order gated before a rep counts.

use doa::blockset::{BlockSet, Deltas};
use sorted_vec::SortedSet;
use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::hint::black_box;
use std::time::{Duration, Instant};

const DEFAULT_N: usize = 10_000;
const CHUNK_LEN: usize = 100;
const RUNS: usize = 10;
///wall clock per structure × workload cell
const RUN_BUDGET: Duration = Duration::from_secs(3);

///32-byte item — orders by key
#[derive(Clone, Copy, Debug)]
struct Item {
    key: u64,
    _pad: [u8; 24],
}

impl Item {
    fn new(key: u64) -> Self {
        Item { key, _pad: [0; 24] }
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

///inline archive — Item is its own handle (32-byte POD)
impl doa::serialize::Archive for Item {
    type Handle = Item;
    type View<'h> = &'h Item;

    fn archive(k: &Self, _heap: &mut Vec<u8>) -> Self::Handle { *k }
    fn cmp_key(h: &Self::Handle, k: &Self, _heap: &[u8]) -> Ordering { h.cmp(k) }
    fn view<'h>(h: &'h Self::Handle, _heap: &'h [u8]) -> Self::View<'h> { h }
    fn materialize(h: Self::Handle, _heap: &[u8]) -> Self { h }
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

///insert all keys (or the prefix the budget allows), ns per insert
fn insert_block(keys: &[u64]) -> Cell {
    reps(Instant::now() + RUN_BUDGET, |deadline| {
        let mut set = BlockSet::<Item, u32>::new();
        let mut deltas = Deltas::new();
        let t = Instant::now();
        let mut done = 0;
        let mut partial = false;
        for &k in keys {
            set.insert(Item::new(k), &mut deltas)
                .unwrap_or_else(|k| panic!("insert failed at {}", k.key));
            deltas.clear();
            done += 1;
            if done & 1023 == 0 && Instant::now() > deadline {
                partial = true;
                break;
            }
        }
        let ns = t.elapsed().as_nanos() as f64 / done as f64;
        assert_eq!(set.len(), done);
        check_order(set.iter().map(|i| i.key));
        black_box(&set);
        (done, partial, ns)
    })
}

fn insert_sortedvec(keys: &[u64]) -> Cell {
    reps(Instant::now() + RUN_BUDGET, |deadline| {
        let mut set = SortedSet::new();
        let t = Instant::now();
        let mut done = 0;
        let mut partial = false;
        for &k in keys {
            let (_, old) = set.replace(Item::new(k));
            assert!(old.is_none(), "dup at {k}");
            done += 1;
            if done & 1023 == 0 && Instant::now() > deadline {
                partial = true;
                break;
            }
        }
        let ns = t.elapsed().as_nanos() as f64 / done as f64;
        assert_eq!(set.len(), done);
        check_order(set.iter().map(|i| i.key));
        black_box(&set);
        (done, partial, ns)
    })
}

fn insert_btree(keys: &[u64]) -> Cell {
    reps(Instant::now() + RUN_BUDGET, |deadline| {
        let mut set = BTreeSet::new();
        let t = Instant::now();
        let mut done = 0;
        let mut partial = false;
        for &k in keys {
            assert!(set.insert(Item::new(k)));
            done += 1;
            if done & 1023 == 0 && Instant::now() > deadline {
                partial = true;
                break;
            }
        }
        let ns = t.elapsed().as_nanos() as f64 / done as f64;
        assert_eq!(set.len(), done);
        check_order(set.iter().map(|i| i.key));
        black_box(&set);
        (done, partial, ns)
    })
}

///probe every key (shuffled probe order), ns per lookup
fn lookup_block(keys: &[u64], probes: &[u64]) -> Cell {
    //workload-order build — the block's layout (Asc/Desc, holes) is the
    //workload's
    let mut set = BlockSet::<Item, u32>::new();
    let mut deltas = Deltas::new();
    for &k in keys {
        set.insert(Item::new(k), &mut deltas).unwrap();
        deltas.clear();
    }
    timed_probes(probes, |item| {
        black_box(set.contains(item));
    })
}

fn lookup_sortedvec(keys: &[u64], probes: &[u64]) -> Cell {
    //sorted-copy build: Vec content is stream-order-independent, and the
    //workload-order build is O(n²)
    let mut sorted = keys.to_vec();
    sorted.sort_unstable();
    let mut set = SortedSet::new();
    for &k in &sorted {
        set.push(Item::new(k));
    }
    timed_probes(probes, |item| {
        black_box(set.binary_search(item).is_ok());
    })
}

fn lookup_btree(keys: &[u64], probes: &[u64]) -> Cell {
    let mut sorted = keys.to_vec();
    sorted.sort_unstable();
    let mut set = BTreeSet::new();
    for &k in &sorted {
        set.insert(Item::new(k));
    }
    timed_probes(probes, |item| {
        black_box(set.contains(item));
    })
}

fn timed_probes(probes: &[u64], mut probe: impl FnMut(&Item)) -> Cell {
    reps(Instant::now() + RUN_BUDGET, |deadline| {
        let t = Instant::now();
        let mut done = 0;
        let mut partial = false;
        for &k in probes {
            probe(&Item::new(k));
            done += 1;
            if done & 8191 == 0 && Instant::now() > deadline {
                partial = true;
                break;
            }
        }
        (done, partial, t.elapsed().as_nanos() as f64 / done as f64)
    })
}

///seq read: iter first→last, ns per element (order gated once, outside
///timing). a pass is ms-scale — the deadline is checked between passes
fn scan_block(keys: &[u64]) -> Cell {
    let mut set = BlockSet::<Item, u32>::new();
    let mut deltas = Deltas::new();
    for &k in keys {
        set.insert(Item::new(k), &mut deltas).unwrap();
        deltas.clear();
    }
    assert_eq!(set.len(), keys.len());
    check_order(set.iter().map(|i| i.key));
    timed_scan(keys.len(), |seed| {
        let mut c = 0u64;
        for i in set.iter() {
            c = c.wrapping_add(i.key ^ seed);
        }
        c
    })
}

fn scan_sortedvec(keys: &[u64]) -> Cell {
    let mut sorted = keys.to_vec();
    sorted.sort_unstable();
    let mut set = SortedSet::new();
    for &k in &sorted {
        set.push(Item::new(k));
    }
    assert_eq!(set.len(), keys.len());
    check_order(set.iter().map(|i| i.key));
    timed_scan(keys.len(), |seed| {
        let mut c = 0u64;
        for i in set.iter() {
            c = c.wrapping_add(i.key ^ seed);
        }
        c
    })
}

fn scan_btree(keys: &[u64]) -> Cell {
    let mut sorted = keys.to_vec();
    sorted.sort_unstable();
    let mut set = BTreeSet::new();
    for &k in &sorted {
        set.insert(Item::new(k));
    }
    assert_eq!(set.len(), keys.len());
    check_order(set.iter().map(|i| i.key));
    timed_scan(keys.len(), |seed| {
        let mut c = 0u64;
        for i in set.iter() {
            c = c.wrapping_add(i.key ^ seed);
        }
        c
    })
}

///one full first→last pass per rep returning a key checksum; the seed
///mixes into each element (xor-then-add — un-factorable) so the invariant
///pass can't be hoisted out of the timing loop
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
    let probes = shuffled(n);
    let workloads: [(&str, Vec<u64>); 4] = [
        ("append", (0..n as u64).collect()),
        ("prepend", (0..n as u64).rev().collect()),
        ("random", shuffled(n)),
        ("runs", runs(n)),
    ];
    for (name, keys) in &workloads {
        let cells = [
            insert_block(keys),
            insert_sortedvec(keys),
            insert_btree(keys),
            lookup_block(keys, &probes),
            lookup_sortedvec(keys, &probes),
            lookup_btree(keys, &probes),
            scan_block(keys),
            scan_sortedvec(keys),
            scan_btree(keys),
        ];
        println!(
            "{name:>8} {n:>9}: insert {} {} {} ns",
            cells[0].fmt("BlockSet", 8),
            cells[1].fmt("SortedSet", 8),
            cells[2].fmt("BTreeSet", 8)
        );
        println!(
            "{name:>8} {n:>9}: lookup {} {} {} ns",
            cells[3].fmt("BlockSet", 7),
            cells[4].fmt("SortedSet", 7),
            cells[5].fmt("BTreeSet", 7)
        );
        println!(
            "{name:>8} {n:>9}: seqread {} {} {} ns/elem",
            cells[6].fmt("BlockSet", 6),
            cells[7].fmt("SortedSet", 6),
            cells[8].fmt("BTreeSet", 6)
        );
    }
}
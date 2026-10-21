//!seek + sequential iteration: the tier cursors vs the std range-based
//!equivalent — `BTreeMap`/`BTreeSet` have no stable cursor (the nightly
//!`lower_bound` API aside), so `range(k..)` forward and `range(..=k).rev()`
//!backward are the surrogate. fwd = seek + RUN_LEN `next`s; back =
//!seek_after + RUN_LEN `prev`s; ns per walk (one seek + up to RUN_LEN
//!steps). see blockmap_insert.rs for the harness: workload-order builds,
//!shuffled probes, 32-byte Item keys, u64 values, cell budget, partial
//!reps. cursor/range walks verified equal over a probe prefix before
//!timing. size via first CLI arg (default 10k, multiple of 100).
//!also a full-scan row per workload: BlockSet `iter` (the SetBits
//!differential) vs a cursor walk over the whole set, fwd (seek min +
//!next) and back (seek_after max + prev) — ns per element.

use doa::blockmap::BlockMap;
use doa::blockset::{BlockSet, Deltas};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::hint::black_box;
use std::time::{Duration, Instant};

const DEFAULT_N: usize = 10_000;
const CHUNK_LEN: usize = 100;
///elements per walk
const RUN_LEN: usize = 100;
const RUNS: usize = 10;
///wall clock per structure × workload cell
const RUN_BUDGET: Duration = Duration::from_secs(3);
///probe prefix the cursor/range walks are verified equal over
const VERIFY: usize = 1000;

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
    const MIN: Self::Handle = Item { key: 0, _pad: [0; 24] };

    fn archive(k: &Self, _heap: &mut Vec<u8>) -> Self::Handle { *k }
    fn cmp_key(h: &Self::Handle, k: &Self, _heap: &[u8]) -> Ordering { h.cmp(k) }
    fn view<'h>(h: &'h Self::Handle, _heap: &'h [u8]) -> Self::View<'h> { h }
    fn materialize(h: Self::Handle, _heap: &[u8]) -> Self { h }
}

///best/avg ns per op over the reps that fit the cell budget. `partial` =
///Some(ops done) when the last rep was cut at the deadline
struct Cell {
    best:     f64,
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

///one rep: every probe is one walk (seek + ≤RUN_LEN steps); the seed
///checksum defeats hoisting. ns per walk over the (possibly cut) prefix
fn timed_walks(probes: &[u64], mut walk: impl FnMut(&Item) -> u64) -> Cell {
    reps(Instant::now() + RUN_BUDGET, |deadline| {
        let t = Instant::now();
        let mut c = 0u64;
        let mut done = 0;
        let mut partial = false;
        for &k in probes {
            c = c.wrapping_add(walk(&Item::new(k)));
            done += 1;
            if done & 63 == 0 && Instant::now() > deadline {
                partial = true;
                break;
            }
        }
        black_box(c);
        (done, partial, t.elapsed().as_nanos() as f64 / done as f64)
    })
}

///seek + RUN_LEN nexts, key+value checksum
fn fwd_map(map: &BlockMap<Item, u64, u32>, probes: &[u64]) -> Cell {
    timed_walks(probes, |item| {
        let mut c = 0u64;
        let mut cur = map.seek(item);
        for _ in 0..RUN_LEN {
            let Some((i, v)) = cur.next() else { break };
            c = c.wrapping_add(i.key ^ v);
        }
        c
    })
}

fn fwd_btree_map(map: &BTreeMap<Item, u64>, probes: &[u64]) -> Cell {
    timed_walks(probes, |item| {
        let mut c = 0u64;
        for (i, v) in map.range(*item..).take(RUN_LEN) {
            c = c.wrapping_add(i.key ^ v);
        }
        c
    })
}

///full scan via the differential Iter — ns per element
fn scan_iter(set: &BlockSet<Item, u32>) -> Cell {
    reps(Instant::now() + RUN_BUDGET, |_| {
        let t = Instant::now();
        let mut c = 0u64;
        for i in set.iter() {
            c = c.wrapping_add(i.key);
        }
        black_box(c);
        (set.len(), false, t.elapsed().as_nanos() as f64 / set.len() as f64)
    })
}

///full scan via cursor: seek min + next to exhaustion
fn scan_cursor(set: &BlockSet<Item, u32>) -> Cell {
    reps(Instant::now() + RUN_BUDGET, |_| {
        let t = Instant::now();
        let mut c = 0u64;
        let mut cur = set.seek(&Item::new(0));
        while let Some(i) = cur.next() {
            c = c.wrapping_add(i.key);
        }
        black_box(c);
        (set.len(), false, t.elapsed().as_nanos() as f64 / set.len() as f64)
    })
}

///full scan via cursor backward: seek_after max + prev to exhaustion
fn scan_cursor_back(set: &BlockSet<Item, u32>, max_key: u64) -> Cell {
    reps(Instant::now() + RUN_BUDGET, |_| {
        let t = Instant::now();
        let mut c = 0u64;
        let mut cur = set.seek_after(&Item::new(max_key));
        while let Some(i) = cur.prev() {
            c = c.wrapping_add(i.key);
        }
        black_box(c);
        (set.len(), false, t.elapsed().as_nanos() as f64 / set.len() as f64)
    })
}

///seek + RUN_LEN nexts, key checksum
fn fwd_set(set: &BlockSet<Item, u32>, probes: &[u64]) -> Cell {
    timed_walks(probes, |item| {
        let mut c = 0u64;
        let mut cur = set.seek(item);
        for _ in 0..RUN_LEN {
            let Some(i) = cur.next() else { break };
            c = c.wrapping_add(i.key);
        }
        c
    })
}

fn fwd_btree_set(set: &BTreeSet<Item>, probes: &[u64]) -> Cell {
    timed_walks(probes, |item| {
        let mut c = 0u64;
        for i in set.range(*item..).take(RUN_LEN) {
            c = c.wrapping_add(i.key);
        }
        c
    })
}

///seek_after + RUN_LEN prevs, key+value checksum
fn back_map(map: &BlockMap<Item, u64, u32>, probes: &[u64]) -> Cell {
    timed_walks(probes, |item| {
        let mut c = 0u64;
        let mut cur = map.seek_after(item);
        for _ in 0..RUN_LEN {
            let Some((i, v)) = cur.prev() else { break };
            c = c.wrapping_add(i.key ^ v);
        }
        c
    })
}

fn back_btree_map(map: &BTreeMap<Item, u64>, probes: &[u64]) -> Cell {
    timed_walks(probes, |item| {
        let mut c = 0u64;
        for (i, v) in map.range(..=*item).rev().take(RUN_LEN) {
            c = c.wrapping_add(i.key ^ v);
        }
        c
    })
}

///seek_after + RUN_LEN prevs, key checksum
fn back_set(set: &BlockSet<Item, u32>, probes: &[u64]) -> Cell {
    timed_walks(probes, |item| {
        let mut c = 0u64;
        let mut cur = set.seek_after(item);
        for _ in 0..RUN_LEN {
            let Some(i) = cur.prev() else { break };
            c = c.wrapping_add(i.key);
        }
        c
    })
}

fn back_btree_set(set: &BTreeSet<Item>, probes: &[u64]) -> Cell {
    timed_walks(probes, |item| {
        let mut c = 0u64;
        for i in set.range(..=*item).rev().take(RUN_LEN) {
            c = c.wrapping_add(i.key);
        }
        c
    })
}

///cursor and range walks must yield the same keys, fwd and back, over
///the probe prefix — the bench's correctness gate
fn verify(name: &str, probes: &[u64], bmap: &BlockMap<Item, u64, u32>,
    bset: &BlockSet<Item, u32>, tmap: &BTreeMap<Item, u64>, tset: &BTreeSet<Item>)
{
    for &k in &probes[..VERIFY.min(probes.len())] {
        let item = Item::new(k);
        let got: Vec<u64> = {
            let mut c = bmap.seek(&item);
            (0..RUN_LEN).filter_map(|_| c.next().map(|(i, _)| i.key)).collect()
        };
        let expect: Vec<u64> = tmap.range(item..).take(RUN_LEN).map(|(i, _)| i.key).collect();
        assert_eq!(got, expect, "{name} map fwd probe {k}");
        let got: Vec<u64> = {
            let mut c = bmap.seek_after(&item);
            (0..RUN_LEN).filter_map(|_| c.prev().map(|(i, _)| i.key)).collect()
        };
        let expect: Vec<u64> =
            tmap.range(..=item).rev().take(RUN_LEN).map(|(i, _)| i.key).collect();
        assert_eq!(got, expect, "{name} map back probe {k}");
        let got: Vec<u64> = {
            let mut c = bset.seek(&item);
            (0..RUN_LEN).filter_map(|_| c.next().map(|i| i.key)).collect()
        };
        let expect: Vec<u64> = tset.range(item..).take(RUN_LEN).map(|i| i.key).collect();
        assert_eq!(got, expect, "{name} set fwd probe {k}");
        let got: Vec<u64> = {
            let mut c = bset.seek_after(&item);
            (0..RUN_LEN).filter_map(|_| c.prev().map(|i| i.key)).collect()
        };
        let expect: Vec<u64> = tset.range(..=item).rev().take(RUN_LEN).map(|i| i.key).collect();
        assert_eq!(got, expect, "{name} set back probe {k}");
    }
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
        //workload-order build — the block layout is the workload's
        let mut bmap = BlockMap::<Item, u64, u32>::new();
        let mut bset = BlockSet::<Item, u32>::new();
        let mut tmap = BTreeMap::new();
        let mut tset = BTreeSet::new();
        let mut deltas = Deltas::new();
        for &k in keys {
            bmap.insert(Item::new(k), k, &mut deltas).unwrap();
            deltas.clear();
            bset.insert(Item::new(k), &mut deltas).unwrap();
            deltas.clear();
            tmap.insert(Item::new(k), k);
            tset.insert(Item::new(k));
        }
        assert_eq!(bmap.len(), keys.len());
        assert_eq!(bset.len(), keys.len());
        verify(name, &probes, &bmap, &bset, &tmap, &tset);
        //scan equality gate: iter and both cursor walks see the same set
        {
            let a: u64 = bset.iter().fold(0u64, |c, i| c.wrapping_add(i.key));
            let mut cur = bset.seek(&Item::new(0));
            let b: u64 =
                std::iter::from_fn(|| cur.next()).fold(0u64, |c, i| c.wrapping_add(i.key));
            let mut cur = bset.seek_after(&Item::new(n as u64 - 1));
            let d: u64 =
                std::iter::from_fn(|| cur.prev()).fold(0u64, |c, i| c.wrapping_add(i.key));
            assert_eq!((a, b, d), (a, a, a), "{name} scan walks disagree");
        }
        let scan = [
            scan_iter(&bset),
            scan_cursor(&bset),
            scan_cursor_back(&bset, n as u64 - 1),
        ];
        let fwd = [
            fwd_set(&bset, &probes),
            fwd_btree_set(&tset, &probes),
            fwd_map(&bmap, &probes),
            fwd_btree_map(&tmap, &probes),
        ];
        let back = [
            back_set(&bset, &probes),
            back_btree_set(&tset, &probes),
            back_map(&bmap, &probes),
            back_btree_map(&tmap, &probes),
        ];
        for (op, cells) in [
            ("fwd100", &fwd),
            ("back100", &back),
        ] {
            println!(
                "{name:>8} {n:>9} {op:>7}: {} {} {} {} ns/walk",
                cells[0].fmt("BlockSet", 8),
                cells[1].fmt("BTreeSet", 8),
                cells[2].fmt("BlockMap", 8),
                cells[3].fmt("BTreeMap", 8)
            );
        }
        println!(
            "{name:>8} {n:>9}    scan: {} {} {} ns/elem",
            scan[0].fmt("iter", 8),
            scan[1].fmt("cursor", 8),
            scan[2].fmt("cur-back", 8),
        );
    }
}
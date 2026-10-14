//!1:1 vs `std::collections::BTreeSet` by workload: sequential append
//!(ascending keys), sequential prepend (descending), random
//!(xorshift-shuffled) — 10k 32-byte items (index-sized payload — element
//!moves pay realistic byte costs), insert + lookup, BlockSet against
//!BTreeSet (prepend measures the wrong-direction cost into an Asc block —
//!the reversal tier's baseline). len + strict order gated before a run
//!counts.

use doa::sortedblock::sortedblock::{BlockSet, Deltas};
use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::hint::black_box;
use std::time::Instant;

const N: usize = 10_000;
const RUNS: usize = 10;

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

fn shuffled() -> Vec<u64> {
    let mut keys: Vec<u64> = (0..N as u64).collect();
    let mut s = 0x2545_F491_4F6C_DD1Du64;
    for i in (1..N).rev() {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        keys.swap(i, (s % (i as u64 + 1)) as usize);
    }
    keys
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

///insert all keys, best/avg ns per insert
fn insert_block(keys: &[u64]) -> (f64, f64) {
    let (mut best, mut sum) = (f64::MAX, 0.0);
    for _ in 0..RUNS {
        let mut set = BlockSet::<Item, u16>::new();
        let mut deltas = Deltas::new();
        let t = Instant::now();
        for &k in keys {
            set.insert(Item::new(k), &mut deltas)
                .unwrap_or_else(|k| panic!("insert failed at {}", k.key));
            deltas.clear();
        }
        let ns = t.elapsed().as_nanos() as f64 / N as f64;
        assert_eq!(set.len(), N);
        check_order(set.iter().map(|i| i.key));
        black_box(&set);
        best = best.min(ns);
        sum += ns;
    }
    (best, sum / RUNS as f64)
}

fn insert_btree(keys: &[u64]) -> (f64, f64) {
    let (mut best, mut sum) = (f64::MAX, 0.0);
    for _ in 0..RUNS {
        let mut set = BTreeSet::new();
        let t = Instant::now();
        for &k in keys {
            assert!(set.insert(Item::new(k)));
        }
        let ns = t.elapsed().as_nanos() as f64 / N as f64;
        assert_eq!(set.len(), N);
        check_order(set.iter().map(|i| i.key));
        black_box(&set);
        best = best.min(ns);
        sum += ns;
    }
    (best, sum / RUNS as f64)
}

///probe every key (shuffled probe order), best/avg ns per lookup
fn lookup_block(keys: &[u64], probes: &[u64]) -> (f64, f64) {
    let mut set = BlockSet::<Item, u16>::new();
    let mut deltas = Deltas::new();
    for &k in keys {
        set.insert(Item::new(k), &mut deltas).unwrap();
        deltas.clear();
    }
    let (mut best, mut sum) = (f64::MAX, 0.0);
    for _ in 0..RUNS {
        let t = Instant::now();
        for &k in probes {
            black_box(set.contains(&Item::new(k)));
        }
        let ns = t.elapsed().as_nanos() as f64 / probes.len() as f64;
        best = best.min(ns);
        sum += ns;
    }
    (best, sum / RUNS as f64)
}

fn lookup_btree(keys: &[u64], probes: &[u64]) -> (f64, f64) {
    let mut set = BTreeSet::new();
    for &k in keys {
        set.insert(Item::new(k));
    }
    let (mut best, mut sum) = (f64::MAX, 0.0);
    for _ in 0..RUNS {
        let t = Instant::now();
        for &k in probes {
            black_box(set.contains(&Item::new(k)));
        }
        let ns = t.elapsed().as_nanos() as f64 / probes.len() as f64;
        best = best.min(ns);
        sum += ns;
    }
    (best, sum / RUNS as f64)
}

fn main() {
    let probes = shuffled();
    let workloads: [(&str, Vec<u64>); 3] = [
        ("append", (0..N as u64).collect()),
        ("prepend", (0..N as u64).rev().collect()),
        ("random", shuffled()),
    ];
    for (name, keys) in &workloads {
        let (bi, ba) = insert_block(keys);
        let (ti, ta) = insert_btree(keys);
        let (bl, bla) = lookup_block(keys, &probes);
        let (tl, tla) = lookup_btree(keys, &probes);
        println!(
            "{name:>8}: insert BlockSet {bi:7.1} (avg {ba:6.1}) BTreeSet {ti:7.1} (avg {ta:6.1}) \
| lookup BlockSet {bl:6.1} (avg {bla:6.1}) BTreeSet {tl:6.1} (avg {tla:6.1}) ns"
        );
    }
}
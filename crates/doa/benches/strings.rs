//!string-keyed mirror of sorted_insert.rs: same workloads (append/prepend/
//!random/runs), insert + lookup + seq-read, `BlockSet<String>` vs
//!`sorted_vec::SortedSet<String>` vs `BTreeSet<String>`. three key shapes:
//!fixed16 — `{:016}` padded (exceeds the StrHandle prefix, so bsearch pays
//!the heap chase); var — `key{n}` (≤ 8 bytes at small n, prefix-resolved
//!compares only); digits — `n.to_string()` (inline at every bench size —
//!the cutoff shape). size via first CLI arg (default 10k, multiple of
//!100). each structure × workload cell capped at RUN_BUDGET (partial reps
//!reported over the completed prefix).

use doa::blockset::{BlockSet, Deltas};
use sorted_vec::SortedSet;
use std::collections::BTreeSet;
use std::hint::black_box;
use std::time::{Duration, Instant};

const DEFAULT_N: usize = 10_000;
const CHUNK_LEN: usize = 100;
const RUNS: usize = 10;
///wall clock per structure × workload cell
const RUN_BUDGET: Duration = Duration::from_secs(3);

///best/avg ns per op over the reps that fit the cell budget
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

///rep driver: reps until RUNS or the deadline
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

///chunks of 100 sequential keys; even chunks reversed; chunk order shuffled
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

///strictly ascending (full string compare)
fn check_order(keys: impl Iterator<Item: AsRef<str>>) {
    let mut prev: Option<String> = None;
    for k in keys {
        let k: &str = k.as_ref();
        if let Some(p) = prev.as_deref() {
            assert!(k > p, "order broken: {k} after {p}");
        }
        prev = Some(k.to_string());
    }
}

///len + first byte, un-hoistable via the seed
fn checksum(items: impl Iterator<Item: AsRef<str>>, seed: u64) -> u64 {
    let mut c = 0u64;
    for s in items {
        let s: &str = s.as_ref();
        c = c
            .wrapping_add(s.len() as u64 ^ seed)
            .wrapping_add(s.as_bytes().first().map_or(0, |b| *b as u64));
    }
    c
}

///insert all keys, ns per insert
fn insert_block(keys: &[String]) -> Cell {
    reps(Instant::now() + RUN_BUDGET, |deadline| {
        let mut set = BlockSet::<String, u32>::new();
        let mut deltas = Deltas::new();
        let t = Instant::now();
        let mut done = 0;
        let mut partial = false;
        for k in keys {
            set.insert(k.clone(), &mut deltas)
                .unwrap_or_else(|k| panic!("insert failed at {k}"));
            deltas.clear();
            done += 1;
            if done & 1023 == 0 && Instant::now() > deadline {
                partial = true;
                break;
            }
        }
        let ns = t.elapsed().as_nanos() as f64 / done as f64;
        assert_eq!(set.len(), done);
        check_order(set.iter());
        black_box(&set);
        (done, partial, ns)
    })
}

fn insert_sortedvec(keys: &[String]) -> Cell {
    reps(Instant::now() + RUN_BUDGET, |deadline| {
        let mut set = SortedSet::new();
        let t = Instant::now();
        let mut done = 0;
        let mut partial = false;
        for k in keys {
            let (_, old) = set.replace(k.clone());
            assert!(old.is_none(), "dup at {k}");
            done += 1;
            if done & 1023 == 0 && Instant::now() > deadline {
                partial = true;
                break;
            }
        }
        let ns = t.elapsed().as_nanos() as f64 / done as f64;
        assert_eq!(set.len(), done);
        check_order(set.iter());
        black_box(&set);
        (done, partial, ns)
    })
}

fn insert_btree(keys: &[String]) -> Cell {
    reps(Instant::now() + RUN_BUDGET, |deadline| {
        let mut set = BTreeSet::new();
        let t = Instant::now();
        let mut done = 0;
        let mut partial = false;
        for k in keys {
            assert!(set.insert(k.clone()));
            done += 1;
            if done & 1023 == 0 && Instant::now() > deadline {
                partial = true;
                break;
            }
        }
        let ns = t.elapsed().as_nanos() as f64 / done as f64;
        assert_eq!(set.len(), done);
        check_order(set.iter());
        black_box(&set);
        (done, partial, ns)
    })
}

///probe every key (shuffled order), ns per lookup
fn lookup_block(keys: &[String], probes: &[String]) -> Cell {
    //workload-order build — the block's layout is the workload's
    let mut set = BlockSet::<String, u32>::new();
    let mut deltas = Deltas::new();
    for k in keys {
        set.insert(k.clone(), &mut deltas).unwrap();
        deltas.clear();
    }
    timed_probes(probes, |p| {
        black_box(set.contains(p));
    })
}

fn lookup_sortedvec(keys: &[String], probes: &[String]) -> Cell {
    //sorted-copy build: the workload-order build is O(n²)
    let mut sorted = keys.to_vec();
    sorted.sort_unstable();
    let mut set = SortedSet::new();
    for k in &sorted {
        set.push(k.clone());
    }
    timed_probes(probes, |p| {
        black_box(set.binary_search(p).is_ok());
    })
}

fn lookup_btree(keys: &[String], probes: &[String]) -> Cell {
    let mut sorted = keys.to_vec();
    sorted.sort_unstable();
    let mut set = BTreeSet::new();
    for k in &sorted {
        set.insert(k.clone());
    }
    timed_probes(probes, |p| {
        black_box(set.contains(p));
    })
}

fn timed_probes(probes: &[String], mut probe: impl FnMut(&String)) -> Cell {
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

///seq read: iter first→last, ns per element
fn scan_block(keys: &[String]) -> Cell {
    let mut set = BlockSet::<String, u32>::new();
    let mut deltas = Deltas::new();
    for k in keys {
        set.insert(k.clone(), &mut deltas).unwrap();
        deltas.clear();
    }
    assert_eq!(set.len(), keys.len());
    check_order(set.iter());
    timed_scan(keys.len(), |seed| checksum(set.iter(), seed))
}

fn scan_sortedvec(keys: &[String]) -> Cell {
    let mut sorted = keys.to_vec();
    sorted.sort_unstable();
    let mut set = SortedSet::new();
    for k in &sorted {
        set.push(k.clone());
    }
    assert_eq!(set.len(), keys.len());
    check_order(set.iter());
    timed_scan(keys.len(), |seed| checksum(set.iter(), seed))
}

fn scan_btree(keys: &[String]) -> Cell {
    let mut sorted = keys.to_vec();
    sorted.sort_unstable();
    let mut set = BTreeSet::new();
    for k in &sorted {
        set.insert(k.clone());
    }
    assert_eq!(set.len(), keys.len());
    check_order(set.iter());
    timed_scan(keys.len(), |seed| checksum(set.iter(), seed))
}

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
    let probe_ids = shuffled(n);
    let shapes: [(&str, fn(u64) -> String); 3] = [
        ("fixed16", |k| format!("{k:016}")),
        ("var", |k| format!("key{k}")),
        //inline at every bench size (≤ 7 bytes at 1M) — the cutoff shape
        ("digits", |k| k.to_string()),
    ];
    for (shape, keyfn) in shapes {
        let workloads: [(&str, Vec<u64>); 4] = [
            ("append", (0..n as u64).collect()),
            ("prepend", (0..n as u64).rev().collect()),
            ("random", shuffled(n)),
            ("runs", runs(n)),
        ];
        let probes: Vec<String> = probe_ids.iter().map(|&k| keyfn(k)).collect();
        for (name, ids) in &workloads {
            let keys: Vec<String> = ids.iter().map(|&k| keyfn(k)).collect();
            //lookup correctness gate before timing (skipset.rs's pattern):
            //BlockSet<String> vs a BTreeSet over a probe prefix — a
            //String-cmp regression must not time silently-wrong lookups
            {
                let mut set = BlockSet::<String, u32>::new();
                let mut deltas = Deltas::new();
                let mut model = BTreeSet::new();
                for key in &keys {
                    set.insert(key.clone(), &mut deltas).unwrap();
                    deltas.clear();
                    model.insert(key.clone());
                }
                for probe in probes.iter().take(1000) {
                    assert_eq!(
                        set.contains(probe),
                        model.contains(probe),
                        "{shape} {name} disagrees at {probe}"
                    );
                }
            }
            let cells = [
                insert_block(&keys),
                insert_sortedvec(&keys),
                insert_btree(&keys),
                lookup_block(&keys, &probes),
                lookup_sortedvec(&keys, &probes),
                lookup_btree(&keys, &probes),
                scan_block(&keys),
                scan_sortedvec(&keys),
                scan_btree(&keys),
            ];
            println!(
                "{shape} {name:>8} {n:>9}: insert {} {} {} ns",
                cells[0].fmt("BlockSet", 8),
                cells[1].fmt("SortedSet", 8),
                cells[2].fmt("BTreeSet", 8)
            );
            println!(
                "{shape} {name:>8} {n:>9}: lookup {} {} {} ns",
                cells[3].fmt("BlockSet", 7),
                cells[4].fmt("SortedSet", 7),
                cells[5].fmt("BTreeSet", 7)
            );
            println!(
                "{shape} {name:>8} {n:>9}: seqread {} {} {} ns/elem",
                cells[6].fmt("BlockSet", 6),
                cells[7].fmt("SortedSet", 6),
                cells[8].fmt("BTreeSet", 6)
            );
        }
    }
}
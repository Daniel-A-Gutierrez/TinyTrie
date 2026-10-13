//!end-to-end insert cost: 10k sequential keys, xorshift-shuffled, inserted
//!into a fresh `BlockSet<u64, u16>` — bsearch + try_alloc + slides + deltas
//!+ grows, all in the timing loop. len + strict order gated before a run
//!counts.

use doa::sortedblock::sortedblock::{BlockSet, Deltas};
use std::hint::black_box;
use std::time::Instant;

const N: usize = 10_000;
const RUNS: usize = 10;

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

fn run(keys: &[u64]) -> f64 {
    let mut set = BlockSet::<u64, u16>::new();
    let mut deltas = Deltas::new();
    let t = Instant::now();
    for &k in keys {
        if set.insert(k, &mut deltas).is_err() {
            panic!("insert failed at {k}");
        }
    }
    let ns = t.elapsed().as_nanos() as f64;
    assert_eq!(set.len(), N);
    let mut prev = None;
    for k in set.iter() {
        if let Some(p) = prev {
            assert!(*k > p);
        }
        prev = Some(*k);
    }
    black_box(&set);
    ns / N as f64
}

fn main() {
    let keys = shuffled();
    let (mut best, mut sum) = (f64::MAX, 0.0);
    for _ in 0..RUNS {
        let ns = run(&keys);
        best = best.min(ns);
        sum += ns;
    }
    println!(
        "{N} shuffled inserts: best {best:.1} ns/insert, avg {:.1} ns/insert ({RUNS} runs)",
        sum / RUNS as f64
    );
}
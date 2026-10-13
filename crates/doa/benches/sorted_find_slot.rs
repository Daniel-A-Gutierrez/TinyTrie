//!find_slot cost: `VecStore` slot scan vs an occupancy bitmask — the devlog's
//!side-buffer idea (u256 mask, line 223) measured. both sides carry the SAME
//!320-slot occupancy pattern: 256 `(usize, usize)` elements, 64 holes at
//!`(13 + i*41) % 320`. probes hit only occupied slots (find_slot's contract),
//!rel alternates; budgets 16 (the modes' budgeted rung) and 320 (the
//!full-len fallback rung). the mask find mirrors `VecStore::find_slot` exactly
//!(pin None) and is asserted equivalent over every probe × rel × budget
//!before any timing runs.

use doa::Rel;
use doa::index::Pos;
use doa::sortedblock::store::{Store, VecStore};
use std::hint::black_box;
use std::time::Instant;

const SLOTS: usize = 320;
const HOLES: usize = 64;
const ITERS: usize = 1_000_000;

fn hole_at(i: usize) -> usize {
    (13 + i * 41) % SLOTS //41 coprime with 320 ⇒ 64 distinct positions
}

///first q in [a, b) with bit q clear; word-wise, trailing_ones per word.
fn first_zero(mask: &[u64], a: usize, b: usize) -> Option<usize> {
    if a >= b {
        return None;
    }
    let (mut w, mut skip) = (a >> 6, (a & 63) as u32);
    loop {
        let ones = (mask[w] >> skip).trailing_ones() as usize;
        if ones + (skip as usize) < 64 {
            let q = (w << 6) + skip as usize + ones;
            return (q < b).then_some(q);
        }
        w += 1;
        skip = 0;
        if w == mask.len() {
            return None; //all words ones past here
        }
    }
}

///last q in [a, b) with bit q clear; word-wise, leading_ones per word.
fn last_zero(mask: &[u64], a: usize, b: usize) -> Option<usize> {
    if a >= b {
        return None;
    }
    let (mut w, mut keep) = ((b - 1) >> 6, 63 - (((b - 1) & 63) as u32));
    loop {
        let ones = (mask[w] << keep).leading_ones() as usize;
        if ones + (keep as usize) < 64 {
            let q = (w << 6) + 63 - ones - (keep as usize);
            return (q >= a).then_some(q);
        }
        w = w.checked_sub(1)?;
        keep = 0;
    }
}

///`VecStore::find_slot` over a bitmask (pin None): rel side first, other side
///on exhaustion. returns (None's pos, to) — same (from, to) the store hands
///back: `to` adjacent-inserting on the rel side, else `pos` (probe shifts
///toward the hole).
fn mask_find_slot(
    mask: &[u64],
    len: usize,
    pos: usize,
    rel: Rel,
    budget: usize,
) -> Option<(usize, usize)> {
    let (min, max) = (pos.saturating_sub(budget), len.min(pos + budget));
    match rel {
        Rel::After => first_zero(mask, pos + 1, max)
            .map(|q| (q, pos + 1))
            .or_else(|| last_zero(mask, min, pos).map(|q| (q, pos))),
        Rel::Before => last_zero(mask, min, pos)
            .map(|q| (q, pos - 1))
            .or_else(|| first_zero(mask, pos + 1, max).map(|q| (q, pos))),
    }
}

fn build() -> (VecStore<(usize, usize)>, Vec<u64>, Vec<usize>) {
    let mut store = VecStore::with_capacity(SLOTS);
    let mut mask = vec![0u64; SLOTS / 64];
    let mut probes = Vec::with_capacity(SLOTS - HOLES);
    for p in 0..SLOTS {
        let hole = (0..HOLES).any(|i| hole_at(i) == p);
        if hole {
            continue;
        }
        store.insert(Pos(p), (p, p));
        mask[p >> 6] |= 1 << (p & 63);
        probes.push(p);
    }
    debug_assert_eq!(store.occupied(), SLOTS - HOLES);
    debug_assert_eq!(probes.len(), SLOTS - HOLES);
    (store, mask, probes)
}

///every probe × rel × budget: the two finds must agree exactly.
fn assert_equivalent(store: &VecStore<(usize, usize)>, mask: &[u64], probes: &[usize]) {
    for budget in [16, SLOTS] {
        for rel in [Rel::After, Rel::Before] {
            for &p in probes {
                let s = store.find_slot(Pos(p), rel, budget, None);
                let m = mask_find_slot(mask, SLOTS, p, rel, budget);
                let (s, m) = (s.map(|ns| (ns.from.0, ns.to.0)), m);
                assert_eq!(s, m, "disagreement at pos {p} {rel:?} budget {budget}");
            }
        }
    }
}

fn time_store(store: &VecStore<(usize, usize)>, probes: &[usize], budget: usize) -> f64 {
    let mut acc = 0usize;
    let t = Instant::now();
    for i in 0..ITERS {
        let p = probes[i % probes.len()];
        let rel = if i & 1 == 0 { Rel::After } else { Rel::Before };
        if let Some(ns) = store.find_slot(Pos(p), rel, budget, None) {
            acc ^= ns.from.0;
        }
    }
    black_box(acc);
    t.elapsed().as_nanos() as f64 / ITERS as f64
}

fn time_mask(mask: &[u64], probes: &[usize], budget: usize) -> f64 {
    let mut acc = 0usize;
    let t = Instant::now();
    for i in 0..ITERS {
        let p = probes[i % probes.len()];
        let rel = if i & 1 == 0 { Rel::After } else { Rel::Before };
        if let Some((q, _)) = mask_find_slot(mask, SLOTS, p, rel, budget) {
            acc ^= q;
        }
    }
    black_box(acc);
    t.elapsed().as_nanos() as f64 / ITERS as f64
}

fn main() {
    let (store, mask, probes) = build();
    assert_equivalent(&store, &mask, &probes);
    println!("slots {SLOTS}, elements {}, holes {HOLES}, slot = (usize, usize)", SLOTS - HOLES);
    for budget in [16, SLOTS] {
        let s = time_store(&store, &probes, budget);
        let m = time_mask(&mask, &probes, budget);
        println!(
            "budget {budget:>3}: store {s:6.2} ns/find, mask {m:6.2} ns/find, ratio {:.2}x",
            s / m
        );
    }
}

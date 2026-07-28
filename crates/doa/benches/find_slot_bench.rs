#![feature(test)]
//! `find_slot` bidirectional vs DIR-biased, at 1M slots, 128-byte `Option<T>`
//! (niche: None ⇒ first u64 0; 128 MiB working set ≫ L3 ⇒ memory-bound).
//! 37.5% (floor) and 90% (search-dominated) occupancy, DIR=true (right/forward —
//! where biased shines). combo = find + slide + undo-slide (store-invariant) ⇒
//! find + 2×slide; find+slide = (find_only + combo)/2.
//!
//! store.rs pulled in via #[path] (items are pub(crate)); same source, no API change.
//! Run: cargo +nightly bench -p doa --bench find_slot_bench
extern crate test;
#[path = "../src/store.rs"]
mod store;
use store::{NoneSlide, Store};
use std::num::NonZeroU64;
use test::{Bencher, black_box};

struct Elem(NonZeroU64, [u64; 15]);
const _: () = assert!(std::mem::size_of::<Option<Elem>>() == 128);
fn some() -> Elem { Elem(NonZeroU64::new(1).unwrap(), [0; 15]) }

const N: usize = 1 << 20;
const BUDGET: usize = N;
const INNER: usize = 256;
const PROBES: usize = 4096;

fn splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E3779B97F4A7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^ (z >> 31)
}

fn flags(n_some: usize) -> Vec<bool> {
    let mut f = vec![false; N];
    for i in 0..n_some { f[i] = true; }
    let mut s = 0xDEAD_BEEF_CAFE_BABE;
    for i in (1..N).rev() {
        let j = (splitmix(&mut s) % (i as u64 + 1)) as usize;
        f.swap(i, j);
    }
    f
}

fn positions() -> Vec<usize> {
    let mut s = 0x0123_4567_89AB_CDEF;
    (0..PROBES).map(|_| (splitmix(&mut s) % N as u64) as usize).collect()
}

fn build_store<'a, S: Store<'a, Elem>>(fl: &[bool]) -> S {
    let mut st = S::new();
    for &on in fl {
        if on { st.push_back(some()); } else { st.grow_back(1); }
    }
    st
}

fn build_vec(fl: &[bool]) -> Vec<Option<Elem>> {
    fl.iter().map(|&on| if on { Some(some()) } else { None }).collect()
}

macro_rules! bench_control {
    ($name:ident, $permille:expr) => {
        #[bench]
        fn $name(b: &mut Bencher) {
            let buf = build_vec(&flags(N * $permille / 1000));
            let pos = positions();
            let mut i = 0usize;
            b.iter(|| {
                let mut acc = 0u64;
                for _ in 0..INNER {
                    let p = black_box(pos[i]);
                    i = (i + 1) & (PROBES - 1);
                    let r = buf[p..].iter().position(|o| o.is_none());
                    acc = acc.wrapping_add(r.map_or(0, |x| (x + p) as u64));
                    black_box(acc);
                }
                acc
            });
        }
    };
}

// `$method` is the Store method to call: find_slot (bidirectional) or find_slot_biased.
macro_rules! bench_find {
    ($name:ident, $permille:expr, $sty:ty, $method:ident) => {
        #[bench]
        fn $name(b: &mut Bencher) {
            let st: $sty = build_store(&flags(N * $permille / 1000));
            let pos = positions();
            let mut i = 0usize;
            b.iter(|| {
                let mut acc = 0u64;
                for _ in 0..INNER {
                    let p = black_box(pos[i]);
                    i = (i + 1) & (PROBES - 1);
                    let r = st.$method(p, black_box(true), black_box(BUDGET), black_box(None));
                    acc = acc.wrapping_add(r.map_or(0, |ms: NoneSlide| ms.from as u64));
                    black_box(acc);
                }
                acc
            });
        }
    };
}

//find + slide + undo-slide: store-invariant ⇒ measures find + 2×slide on a stable dist.
macro_rules! bench_combo {
    ($name:ident, $permille:expr, $sty:ty, $method:ident) => {
        #[bench]
        fn $name(b: &mut Bencher) {
            let mut st: $sty = build_store(&flags(N * $permille / 1000));
            let pos = positions();
            let mut i = 0usize;
            b.iter(|| {
                let mut acc = 0u64;
                for _ in 0..INNER {
                    let p = black_box(pos[i]);
                    i = (i + 1) & (PROBES - 1);
                    let r = st.$method(p, black_box(true), black_box(BUDGET), black_box(None));
                    if let Some(ms) = r {
                        let (f, t) = (ms.from, ms.to);
                        st.slide_none(ms, black_box(None));
                        st.slide_none(NoneSlide { from: t, to: f }, black_box(None));
                        acc = acc.wrapping_add(f as u64);
                    }
                    black_box(acc);
                }
                acc
            });
        }
    };
}

bench_control!(control_375, 375);
bench_control!(control_900, 900);

bench_find!(vec_bi_375, 375, store::VecStore<Elem, N>, find_nearest_slot);
bench_find!(vec_bi_900, 900, store::VecStore<Elem, N>, find_nearest_slot);
bench_find!(vec_bia_375, 375, store::VecStore<Elem, N>, find_slot);
bench_find!(vec_bia_900, 900, store::VecStore<Elem, N>, find_slot);

bench_find!(deq_bi_375, 375, store::DequeStore<Elem, N>, find_nearest_slot);
bench_find!(deq_bi_900, 900, store::DequeStore<Elem, N>, find_nearest_slot);
bench_find!(deq_bia_375, 375, store::DequeStore<Elem, N>, find_slot);
bench_find!(deq_bia_900, 900, store::DequeStore<Elem, N>, find_slot);

bench_combo!(combo_vec_bi_375, 375, store::VecStore<Elem, N>, find_nearest_slot);
bench_combo!(combo_vec_bi_900, 900, store::VecStore<Elem, N>, find_nearest_slot);
bench_combo!(combo_vec_bia_375, 375, store::VecStore<Elem, N>, find_slot);
bench_combo!(combo_vec_bia_900, 900, store::VecStore<Elem, N>, find_slot);

bench_combo!(combo_deq_bi_375, 375, store::DequeStore<Elem, N>, find_nearest_slot);
bench_combo!(combo_deq_bi_900, 900, store::DequeStore<Elem, N>, find_nearest_slot);
bench_combo!(combo_deq_bia_375, 375, store::DequeStore<Elem, N>, find_slot);
bench_combo!(combo_deq_bia_900, 900, store::DequeStore<Elem, N>, find_slot);
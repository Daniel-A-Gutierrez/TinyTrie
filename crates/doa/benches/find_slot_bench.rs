#![feature(test)]
//! `Store::find_slot` vs a naive forward `Vec<Option>` scan, at 1M slots and
//! {37.5, 50, 75, 90}% occupancy. find_slot is bidirectional-outward (nearest
//! None); the control is a one-directional `position(is_none)` from a random
//! index — the naive insert's cost. budget is unbounded (== N) so find_slot
//! always finds the nearest None; the scan length is then set by occupancy.
//!
//! store.rs is pulled in via #[path] (its items are pub(crate)); same source,
//! identical codegen, no doa API change.
//!
//! Run: cargo +nightly bench -p doa --bench find_slot_bench
extern crate test;
#[path = "../src/store.rs"]
mod store;
use store::{MinSlide, Store};
use test::{Bencher, black_box};

const N: usize = 1 << 20; // 1,048,576 slots
const BUDGET: usize = N; // unbounded: find the nearest None, not budget-capped
const INNER: usize = 256; // finds per timed iter
const PROBES: usize = 4096; // precomputed random positions (pow2 for &-mask)

//splitmix64 — deterministic, no dep.
fn splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E3779B97F4A7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^ (z >> 31)
}

//N slots, `n_some` occupied at pseudo-random positions (fisher-yates).
fn flags(n_some: usize) -> Vec<bool> {
    let mut f = vec![false; N];
    for i in 0..n_some {
        f[i] = true;
    }
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

//build a Store from a flag pattern by appending Some (push_back) / None (grow_back).
fn build_store<'a, S: Store<'a, u64>>(fl: &[bool]) -> S {
    let mut st = S::new();
    for &on in fl {
        if on {
            st.push_back(0u64);
        } else {
            st.grow_back(1);
        }
    }
    st
}

fn build_vec(fl: &[bool]) -> Vec<Option<u64>> {
    fl.iter().map(|&on| if on { Some(0u64) } else { None }).collect()
}

//occupancy permille: 375 / 500 / 750 / 900.
macro_rules! bench_control {
    ($name:ident, $permille:expr) => {
        #[bench]
        fn $name(b: &mut Bencher) {
            let n_some = N * $permille / 1000;
            let buf = build_vec(&flags(n_some));
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

macro_rules! bench_find {
    ($name:ident, $permille:expr, $sty:ty, $dir:literal) => {
        #[bench]
        fn $name(b: &mut Bencher) {
            let n_some = N * $permille / 1000;
            let st: $sty = build_store(&flags(n_some));
            let pos = positions();
            let mut i = 0usize;
            b.iter(|| {
                let mut acc = 0u64;
                for _ in 0..INNER {
                    let p = black_box(pos[i]);
                    i = (i + 1) & (PROBES - 1);
                    let r = st.find_slot::<$dir>(p, black_box(BUDGET), black_box(None));
                    acc = acc.wrapping_add(r.map_or(0, |ms: MinSlide| ms.from as u64));
                    black_box(acc);
                }
                acc
            });
        }
    };
}

// ---- control: naive forward Vec<Option> scan for first None after pos ----
bench_control!(control_375, 375);
bench_control!(control_500, 500);
bench_control!(control_750, 750);
bench_control!(control_900, 900);

// ---- VecStore::find_slot, right(after) / left(before) ----
bench_find!(vec_r_375, 375, store::VecStore<u64, N>, true);
bench_find!(vec_r_500, 500, store::VecStore<u64, N>, true);
bench_find!(vec_r_750, 750, store::VecStore<u64, N>, true);
bench_find!(vec_r_900, 900, store::VecStore<u64, N>, true);
bench_find!(vec_l_375, 375, store::VecStore<u64, N>, false);
bench_find!(vec_l_500, 500, store::VecStore<u64, N>, false);
bench_find!(vec_l_750, 750, store::VecStore<u64, N>, false);
bench_find!(vec_l_900, 900, store::VecStore<u64, N>, false);

// ---- DequeStore::find_slot, right / left ----
bench_find!(deq_r_375, 375, store::DequeStore<u64, N>, true);
bench_find!(deq_r_500, 500, store::DequeStore<u64, N>, true);
bench_find!(deq_r_750, 750, store::DequeStore<u64, N>, true);
bench_find!(deq_r_900, 900, store::DequeStore<u64, N>, true);
bench_find!(deq_l_375, 375, store::DequeStore<u64, N>, false);
bench_find!(deq_l_500, 500, store::DequeStore<u64, N>, false);
bench_find!(deq_l_750, 750, store::DequeStore<u64, N>, false);
bench_find!(deq_l_900, 900, store::DequeStore<u64, N>, false);
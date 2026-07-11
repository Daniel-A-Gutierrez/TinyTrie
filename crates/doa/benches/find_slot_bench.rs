#![feature(test)]
//! Native `cargo bench` target for `find_twophase`.
//!
//! Sweeps {scenario} × {stride}; scenarios fix the answer and probe count so
//! per-probe overhead differences surface.
//!
//! Run: `cargo bench -p doa --bench find_slot_bench`

extern crate test;

use doa::find_slot::{find_twophase, Found, Params};
use test::{black_box, Bencher};

/// Element type under test. `u64` = small/SIMD-friendly; `[u64; 8]` ≈ a tree
/// node (64 B payload, ~72 B with the `Option` discriminant). Flip this alias
/// to compare cache footprints.
type Elem = [u64; 8];
const SOME: Elem = [0u64; 8];

const LEN: usize = 4096;
const BUDGET: usize = 16;

fn params(stride: usize) -> Params {
    Params {
        none_mask: stride - 1,
        v_off_phys: 3,
        budget: BUDGET,
        addr_shift: 0,
        v_offset: 3,
        addr_min: i16::MIN as isize,
        addr_max: i16::MAX as isize,
    }
}

fn blank(len: usize) -> Vec<Option<Elem>> {
    vec![Some(SOME); len]
}

/// Eligible AP around `h`; set `None` at the `rank`-th slot on each side.
fn place_none_at_rank(buf: &mut [Option<Elem>], h: usize, par: &Params, rank: usize) {
    let stride = par.stride() as isize;
    let r = (h.wrapping_add(par.v_off_phys)) & par.none_mask;
    let up_delta = (1usize.wrapping_sub(r)) & par.none_mask;
    let up = h as isize + up_delta as isize;
    let down = up - stride;
    let ri = (up + stride * rank as isize) as usize;
    if ri < buf.len() {
        buf[ri] = None;
    }
    let li = (down - stride * rank as isize) as isize;
    if li >= 0 {
        buf[li as usize] = None;
    }
}

fn case_internal(stride: usize, rank: usize) -> (Vec<Option<Elem>>, usize, Params) {
    let par = params(stride);
    let mut buf = blank(LEN);
    let h = LEN / 2;
    place_none_at_rank(&mut buf, h, &par, rank);
    (buf, h, par)
}

fn case_append(stride: usize) -> (Vec<Option<Elem>>, usize, Params) {
    let par = params(stride);
    let buf = blank(LEN);
    let h = LEN - BUDGET / 2 * stride; // right reaches back in ~budget/2 probes
    (buf, h, par)
}

fn case_prepend(stride: usize) -> (Vec<Option<Elem>>, usize, Params) {
    let par = params(stride);
    let buf = blank(LEN);
    let h = BUDGET / 2 * stride; // left reaches front in ~budget/2 probes
    (buf, h, par)
}

fn case_miss(stride: usize) -> (Vec<Option<Elem>>, usize, Params) {
    let par = params(stride);
    let buf = blank(LEN);
    let h = LEN / 2; // no None, ends out of reach -> OutOfBudget
    (buf, h, par)
}

macro_rules! bench_internal {
    ($name:ident, $strat:ident, $stride:expr, $rank:expr) => {
        #[bench]
        fn $name(b: &mut Bencher) {
            let (buf, h, p) = case_internal($stride, $rank);
            b.iter(|| {
                let r = $strat(black_box(&buf), black_box(h), black_box(&p));
                black_box(r)
            });
        }
    };
}

macro_rules! bench_case {
    ($name:ident, $strat:ident, $case:expr, $stride:expr) => {
        #[bench]
        fn $name(b: &mut Bencher) {
            let (buf, h, p) = $case($stride);
            b.iter(|| {
                let r = $strat(black_box(&buf), black_box(h), black_box(&p));
                black_box(r)
            });
        }
    };
}

// ---- twophase: near (rank 1) / far (rank 14) ----
bench_internal!(twophase_near_s2, find_twophase, 2usize, 1);
bench_internal!(twophase_far_s2, find_twophase, 2usize, 14);
bench_internal!(twophase_near_s8, find_twophase, 8usize, 1);
bench_internal!(twophase_far_s8, find_twophase, 8usize, 14);

// ---- append / prepend / miss (stride 2 & 8) ----
bench_case!(twophase_append_s2, find_twophase, case_append, 2usize);
bench_case!(twophase_prepend_s2, find_twophase, case_prepend, 2usize);
bench_case!(twophase_miss_s2, find_twophase, case_miss, 2usize);
bench_case!(twophase_append_s8, find_twophase, case_append, 8usize);
bench_case!(twophase_prepend_s8, find_twophase, case_prepend, 8usize);
bench_case!(twophase_miss_s8, find_twophase, case_miss, 8usize);

// ---- streaming miss (cold cache, >L3): cheap per-probe when memory-bound? ----
macro_rules! bench_stream {
    ($name:ident, $strat:ident, $stride:expr) => {
        #[bench]
        fn $name(b: &mut Bencher) {
            let p = params($stride);
            let buf = blank(1 << 18); // 262144 * 72 B ~= 18.9 MiB > L3
            let len = buf.len();
            const INNER: usize = 256;
            b.iter(|| {
                let mut h = 0usize;
                let mut acc = 0u64;
                for _ in 0..INNER {
                    let r = $strat(black_box(&buf), black_box(h), black_box(&p));
                    acc = acc.wrapping_add(match r {
                        Ok(Found::InsertA(i)) => i as u64,
                        _ => 0,
                    });
                    h = (h + 263) % len;
                }
                black_box(acc)
            });
        }
    };
}
bench_stream!(stream_twophase_s8, find_twophase, 8usize);

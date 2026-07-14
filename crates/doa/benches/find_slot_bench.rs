#![feature(test)]
//! Native `cargo bench` target for `Block::find_slot`.
//!
//! Sweeps {scenario} × {stride}; scenarios fix the answer and probe count so
//! per-probe overhead differences surface.
//!
//! Run: `cargo bench -p doa --bench find_slot_bench`

extern crate test;

use doa::block::Block;
use doa::find_slot::{Bias, Found};
use std::collections::VecDeque;
use test::{Bencher, black_box};

/// Element type under test. `u64` = small/SIMD-friendly; `[u64; 8]` ≈ a tree
/// node (64 B payload, ~72 B with the `Option` discriminant). Flip this alias
/// to compare cache footprints.
type Elem = [u64; 8];
const SOME: Elem = [0u64; 8];

const LEN: usize = 4096;
const BUDGET: usize = 16;

/// Args tuple: `(none_mask, v_off_phys, budget, addr_shift, v_offset)`.
/// `addr_min`/`addr_max` come from `PTR = i16` inside `Block::find_slot`.
fn args(stride: usize) -> (usize, usize, usize, u32, usize) {
    (stride - 1, 3, BUDGET, 0, 3)
}

fn blank(len: usize) -> VecDeque<Option<Elem>> {
    vec![Some(SOME); len].into()
}

/// Eligible AP around `h`; set `None` at the `rank`-th slot on each side.
fn place_none_at_rank(buf: &mut VecDeque<Option<Elem>>,
                      h: usize,
                      none_mask: usize,
                      v_off_phys: usize,
                      rank: usize) {
    let stride = (none_mask + 1) as isize;
    let r = (h.wrapping_add(v_off_phys)) & none_mask;
    let up_delta = (1usize.wrapping_sub(r)) & none_mask;
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

fn case_internal(stride: usize, rank: usize) -> (VecDeque<Option<Elem>>, usize) {
    let (none_mask, v_off_phys, _, _, _) = args(stride);
    let mut buf = blank(LEN);
    let h = LEN / 2;
    place_none_at_rank(&mut buf, h, none_mask, v_off_phys, rank);
    (buf, h)
}

fn case_append(stride: usize) -> (VecDeque<Option<Elem>>, usize) {
    let buf = blank(LEN);
    let h = LEN - BUDGET / 2 * stride; // right reaches back in ~budget/2 probes
    (buf, h)
}

fn case_prepend(stride: usize) -> (VecDeque<Option<Elem>>, usize) {
    let buf = blank(LEN);
    let h = BUDGET / 2 * stride; // left reaches front in ~budget/2 probes
    (buf, h)
}

fn case_miss(_stride: usize) -> (VecDeque<Option<Elem>>, usize) {
    let buf = blank(LEN);
    let h = LEN / 2; // no None, ends out of reach -> OutOfBudget
    (buf, h)
}

/// Build a `Block<Elem, i16>` from the args tuple's translation fields.
fn make_block(buf: VecDeque<Option<Elem>>,
              addr_shift: u32,
              none_mask: usize,
              v_offset: usize)
              -> Block<Elem, i16> {
    Block::from_raw_parts(buf, addr_shift, none_mask as u32, v_offset as isize).with_budget(BUDGET)
}

macro_rules! bench_internal {
    ($name:ident, $stride:expr, $rank:expr, $bias:expr) => {
        #[bench]
        fn $name(b: &mut Bencher) {
            let (buf, h) = case_internal($stride, $rank);
            let (none_mask, _, _budget, addr_shift, v_offset) = args($stride);
            let block = make_block(buf, addr_shift, none_mask, v_offset);
            b.iter(|| {
                 let r = block.find_slot(black_box(h), $bias);
                 black_box(r)
             });
        }
    };
}

macro_rules! bench_case {
    ($name:ident, $case:expr, $stride:expr) => {
        #[bench]
        fn $name(b: &mut Bencher) {
            let (buf, h) = $case($stride);
            let (none_mask, _, _budget, addr_shift, v_offset) = args($stride);
            let block = make_block(buf, addr_shift, none_mask, v_offset);
            b.iter(|| {
                 let r = block.find_slot(black_box(h), Bias::Right);
                 black_box(r)
             });
        }
    };
}

// ---- find_slot: near (rank 1) / far (rank 14) ----
bench_internal!(find_slot_near_s2, 2usize, 1, Bias::Right);
bench_internal!(find_slot_far_s2, 2usize, 14, Bias::Right);
bench_internal!(find_slot_near_s8, 8usize, 1, Bias::Right);
bench_internal!(find_slot_far_s8, 8usize, 14, Bias::Right);
// Left-bias sweep — exercises the left-first phase-1 loop (separate codegen
// from the right-first path), so its perf is tracked, not just the right path.
bench_internal!(find_slot_near_s2_left, 2usize, 1, Bias::Left);
bench_internal!(find_slot_far_s2_left, 2usize, 14, Bias::Left);
bench_internal!(find_slot_near_s8_left, 8usize, 1, Bias::Left);
bench_internal!(find_slot_far_s8_left, 8usize, 14, Bias::Left);

// ---- append / prepend / miss (stride 2 & 8) ----
bench_case!(find_slot_append_s2, case_append, 2usize);
bench_case!(find_slot_prepend_s2, case_prepend, 2usize);
bench_case!(find_slot_miss_s2, case_miss, 2usize);
bench_case!(find_slot_append_s8, case_append, 8usize);
bench_case!(find_slot_prepend_s8, case_prepend, 8usize);
bench_case!(find_slot_miss_s8, case_miss, 8usize);

// ---- streaming miss (cold cache, >L3): cheap per-probe when memory-bound? ----
macro_rules! bench_stream {
    ($name:ident, $stride:expr) => {
        #[bench]
        fn $name(b: &mut Bencher) {
            let (none_mask, _, _budget, addr_shift, v_offset) = args($stride);
            let buf = blank(1 << 18); // 262144 * 72 B ~= 18.9 MiB > L3
            let len = buf.len();
            let block = make_block(buf, addr_shift, none_mask, v_offset);
            const INNER: usize = 256;
            b.iter(|| {
                 let mut h = 0usize;
                 let mut acc = 0u64;
                 for _ in 0..INNER {
                     let r = block.find_slot(black_box(h), Bias::Right);
                     acc = acc.wrapping_add(match r {
                                                Ok(Found::At(i)) => i as u64,
                                                _ => 0,
                                            });
                     h = (h + 263) % len;
                 }
                 black_box(acc)
             });
        }
    };
}
bench_stream!(stream_find_slot_s8, 8usize);

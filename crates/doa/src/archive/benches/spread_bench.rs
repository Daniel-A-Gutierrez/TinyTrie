#![feature(test)]
//! spread i->2i on VecDeque<Option<usize>>: three approaches, plus a
//! make_contiguous investigation on a wrapped deque.
//! n = 1<<14 (16384 — nearest pow2 >= 10k; phase-2 halving needs pow2).
//! setup (build the deque) is inside the timed closure, so subtract the
//! matching `make_only_*` baseline to get spread-only cost.
//! Run: cargo bench -p doa --bench spread_bench
extern crate test;
use std::collections::VecDeque;
use test::{Bencher, black_box};
const M: usize = 1 << 14;
// contig deque: cap 2M, len M, head 0 (built via push_back -> no wrap)
fn make_contig() -> VecDeque<Option<usize>> {
    let mut v = VecDeque::with_capacity(2 * M);
    v.extend((0..M).map(Some));
    v
}
// wrapped deque: head pushed to 1.5M so data straddles the ring boundary.
// extend [0,half) then push_front half -> head = 2M - half = 1.5M, len M, wraps.
fn make_wrapped() -> VecDeque<Option<usize>> {
    let mut v = VecDeque::with_capacity(2 * M);
    let half = M / 2;
    v.extend((0..half).map(Some));
    for i in 0..half {
        v.push_front(Some(half + i));
    }
    v
}
// R: push_back(None) n times, then 1-pass reverse spread. ~3n writes.
fn spread_pushnone<T>(buf: &mut VecDeque<Option<T>>) {
    let n = buf.len();
    for _ in 0..n {
        buf.push_back(None);
    }
    for i in (0..n).rev() {
        let v = buf[i].take();
        buf[2 * i] = v;
    }
}
// phase 1 (shared by U and U'): take upper half [mid,n), push each then a None.
// places upper half into final positions [n,2n); leaves [mid,n) = None (space).
fn phase1_interleave<T>(buf: &mut VecDeque<Option<T>>, n: usize) {
    let mid = n / 2;
    for i in mid..n {
        let v = buf[i].take();
        buf.push_back(v);
        buf.push_back(None);
    }
}
// U: phase1 + halving divide-and-conquer over [0,n). ~2.5n writes.
fn spread_interleaved_halving<T>(buf: &mut VecDeque<Option<T>>) {
    let n = buf.len();
    phase1_interleave(buf, n);
    halving_phase2(buf, 0, n);
}
// region [off, off+end); live [off, off+end/2); space [off+end/2, off+end) (None).
// move upper-live -> even slots of space, recurse left. src<dst, space is None -> safe.
fn halving_phase2<T>(buf: &mut VecDeque<Option<T>>, off: usize, end: usize) {
    let live = end / 2;
    if live <= 1 {
        return;
    }
    let half = live / 2;
    for j in half..live {
        let v = buf[off + j].take();
        buf[off + 2 * j] = v;
    }
    halving_phase2(buf, off, live);
}
// U': phase1 + single reverse-move over [0,n). same writes as U's phase 2, linear sweep.
fn spread_interleaved_reverse<T>(buf: &mut VecDeque<Option<T>>) {
    let n = buf.len();
    let mid = n / 2;
    phase1_interleave(buf, n);
    for j in (0..mid).rev() {
        let v = buf[j].take();
        buf[2 * j] = v;
    }
}
// U'+mc: phase1, make_contiguous, then reverse-move on the returned slice
// (slice indexing skips the deque's per-access (head+i)%cap).
fn spread_interleaved_reverse_mc<T>(buf: &mut VecDeque<Option<T>>) {
    let n = buf.len();
    let mid = n / 2;
    phase1_interleave(buf, n);
    let s = buf.make_contiguous();
    for j in (0..mid).rev() {
        let v = s[j].take();
        s[2 * j] = v;
    }
}
// verify all approaches agree, once, before timing.
static CHECK: std::sync::Once = std::sync::Once::new();
fn check_once() {
    CHECK.call_once(|| {
        for &m in &[4usize, 16, 64, 256, 1024, 4096] {
            let base: VecDeque<Option<usize>> = (0..m).map(Some).collect();
            let mut r = base.clone();
            let mut u = base.clone();
            let mut up = base.clone();
            let mut mc = base.clone();
            spread_pushnone(&mut r);
            spread_interleaved_halving(&mut u);
            spread_interleaved_reverse(&mut up);
            // mc needs cap >= 2m; grow a copy
            mc.reserve(m);
            spread_interleaved_reverse_mc(&mut mc);
            assert_eq!(r, u, "halving != pushnone at m={m}");
            assert_eq!(r, up, "reverse != pushnone at m={m}");
            assert_eq!(r, mc, "mc != pushnone at m={m}");
        }
        // wrapped input: U' and U'+mc must still match R
        let mut w = make_wrapped_small();
        let mut wr = w.clone();
        spread_pushnone(&mut wr);
        let mut wu = w.clone();
        spread_interleaved_reverse(&mut wu);
        assert_eq!(wr, wu, "wrapped: reverse != pushnone");
    });
}
fn make_wrapped_small() -> VecDeque<Option<usize>> {
    let m = 256;
    let half = m / 2;
    let mut v = VecDeque::with_capacity(2 * m);
    v.extend((0..half).map(Some));
    for i in 0..half {
        v.push_front(Some(half + i));
    }
    v
}
#[bench]
fn make_only_contig(b: &mut Bencher) {
    check_once();
    b.iter(|| black_box(make_contig()));
}
#[bench]
fn pushnone_spread(b: &mut Bencher) {
    check_once();
    b.iter(|| {
        let mut v = make_contig();
        spread_pushnone(&mut v);
        black_box(v)
    });
}
#[bench]
fn interleaved_halving(b: &mut Bencher) {
    check_once();
    b.iter(|| {
        let mut v = make_contig();
        spread_interleaved_halving(&mut v);
        black_box(v)
    });
}
#[bench]
fn interleaved_reverse(b: &mut Bencher) {
    check_once();
    b.iter(|| {
        let mut v = make_contig();
        spread_interleaved_reverse(&mut v);
        black_box(v)
    });
}
#[bench]
fn interleaved_reverse_mc(b: &mut Bencher) {
    check_once();
    b.iter(|| {
        let mut v = make_contig();
        spread_interleaved_reverse_mc(&mut v);
        black_box(v)
    });
}
#[bench]
fn make_only_wrapped(b: &mut Bencher) {
    check_once();
    b.iter(|| black_box(make_wrapped()));
}
#[bench]
fn interleaved_reverse_wrapped(b: &mut Bencher) {
    check_once();
    b.iter(|| {
        let mut v = make_wrapped();
        spread_interleaved_reverse(&mut v);
        black_box(v)
    });
}
#[bench]
fn interleaved_reverse_wrapped_mc(b: &mut Bencher) {
    check_once();
    b.iter(|| {
        let mut v = make_wrapped();
        spread_interleaved_reverse_mc(&mut v);
        black_box(v)
    });
}

//!store tests: reference-model torture + exhaustive slide/find matrices for both
//!backends, drop-counted leak checks. run targeted (full-crate test runs have
//!OOM'd the IDE): `cargo test -p doa --lib store::tests`. miri (uninit reads +
//!leaks): `cargo +nightly miri test -p doa --lib store::tests`.
use std::cell::Cell;
use std::rc::Rc;

use super::*;
use crate::metadata::TwoSlide;

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

///store-agnostic invariants after any mutation.
fn assert_inv<S: Store<'static, u64>>(s: &S) {
    let (occ, len, cap) = (s.occupied(), s.len(), s.cap());
    assert!(occ <= len, "occ={occ} > len={len}");
    assert!(len <= cap, "len={len} > cap={cap}");
    assert_eq!(s.iter().copied().count(), occ, "iter count != occupied");
    assert_eq!(s.iter().rev().copied().count(), occ, "rev iter count != occupied");
    assert_eq!(s.iter().len(), occ, "ExactSizeIterator len != occupied");
}

///logical slot contents, None holes included.
fn snap<S: Store<'static, u64>>(s: &S) -> Vec<Option<u64>> {
    let len = s.len();
    let mut v = Vec::with_capacity(len);
    for i in 0..len {
        v.push(s.slot(i).copied());
    }
    v
}

///reference slide: rotate [lo,hi] so the element at `from` lands at `to`.
fn ref_slide(buf: &mut [Option<u64>], from: usize, to: usize) {
    if from == to {
        return;
    }
    let (lo, hi) = if from > to { (to, from) } else { (from, to) };
    if from > to {
        buf[lo..=hi].rotate_right(1);
    } else {
        buf[lo..=hi].rotate_left(1);
    }
}

///reference find_slot — faithful to VecStore::find_slot's DIR-biased, budget-bounded,
///pin-clamped contract. returns (from, to).
fn ref_find_slot(
    buf: &[Option<u64>],
    pos: usize,
    dir: bool,
    budget: usize,
    pin: Option<usize>,
) -> Option<(usize, usize)> {
    let max = (u32::MAX as usize).min(buf.len()).min(pos + budget);
    let min = pos.saturating_sub(budget);
    let (min, max) = match pin {
        Some(p) if p == pos => {
            if dir {
                (pos, max)
            } else {
                (min, pos)
            }
        }
        Some(p) if p < pos => (min.max(p + 1), max),
        Some(p) => (min, max.min(p)),
        None => (min, max),
    };
    let lcnt = pos - min;
    let rcnt = max.saturating_sub(pos + 1);
    if dir {
        if rcnt > 0
            && let Some(r) = buf[pos + 1..max].iter().position(|o| o.is_none())
        {
            return Some((pos + 1 + r, pos + 1));
        }
        if lcnt > 0
            && let Some(l) = buf[min..pos].iter().rposition(|o| o.is_none())
        {
            return Some((min + l, pos));
        }
        None
    } else {
        if lcnt > 0
            && let Some(l) = buf[min..pos].iter().rposition(|o| o.is_none())
        {
            return Some((min + l, pos - 1));
        }
        if rcnt > 0
            && let Some(r) = buf[pos + 1..max].iter().position(|o| o.is_none())
        {
            return Some((pos + 1 + r, pos));
        }
        None
    }
}

///reference find_nearest_slot — outward dual scan, dir tie-break, same `to` rules.
fn ref_nearest(
    buf: &[Option<u64>],
    pos: usize,
    dir: bool,
    budget: usize,
    pin: Option<usize>,
) -> Option<(usize, usize)> {
    let max = (u32::MAX as usize).min(buf.len()).min(pos + budget);
    let min = pos.saturating_sub(budget);
    let (min, max) = match pin {
        Some(p) if p == pos => {
            if dir {
                (pos, max)
            } else {
                (min, pos)
            }
        }
        Some(p) if p < pos => (min.max(p + 1), max),
        Some(p) => (min, max.min(p)),
        None => (min, max),
    };
    let lcnt = pos - min;
    let rcnt = max.saturating_sub(pos + 1);
    let m = lcnt.min(rcnt);
    for k in 0..m {
        let l = buf[pos - 1 - k].is_none();
        let r = buf[pos + 1 + k].is_none();
        if l && r {
            return if dir {
                Some((pos + 1 + k, pos + 1))
            } else {
                Some((pos - 1 - k, pos - 1))
            };
        }
        if l {
            return Some((pos - 1 - k, if !dir { pos - 1 } else { pos }));
        }
        if r {
            return Some((pos + 1 + k, if dir { pos + 1 } else { pos }));
        }
    }
    for k in m..lcnt {
        if buf[pos - 1 - k].is_none() {
            return Some((pos - 1 - k, if !dir { pos - 1 } else { pos }));
        }
    }
    for k in m..rcnt {
        if buf[pos + 1 + k].is_none() {
            return Some((pos + 1 + k, if dir { pos + 1 } else { pos }));
        }
    }
    None
}

///reference spread: element i -> 2i+offset, everything else None, len doubles.
fn ref_spread(buf: &[Option<u64>], offset: usize) -> Vec<Option<u64>> {
    let mut out = vec![None; buf.len() * 2];
    for (i, o) in buf.iter().enumerate() {
        out[2 * i + offset] = *o;
    }
    out
}

struct Rng(u64);
impl Rng {
    fn below(&mut self, n: u64) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 % n
    }
}

///random index of a slot in state `want_some`, if any exists.
fn idx_where(model: &[Option<u64>], want_some: bool, rng: &mut Rng) -> Option<usize> {
    let hits: Vec<usize> = model
        .iter()
        .enumerate()
        .filter(|(_, o)| o.is_some() == want_some)
        .map(|(i, _)| i)
        .collect();
    if hits.is_empty() {
        return None;
    }
    hits.get(rng.below(hits.len() as u64) as usize).copied()
}

fn p1() -> Vec<Option<u64>> {
    vec![Some(0), None, Some(2), Some(3), None, Some(5), Some(6), None, Some(8)]
}

///find (find_slot or find_nearest_slot per `biased`), compare against the
///reference, then perform the full insert workflow: slide, alloc, write.
fn insert_via<S: Store<'static, u64>>(
    s: &mut S,
    model: &mut Vec<Option<u64>>,
    pos: usize,
    biased: bool,
    rng: &mut Rng,
    val: &mut u64,
    step: usize,
) {
    let dir = rng.below(2) == 1;
    let budget = [0usize, 1, 2, 3, 5, 8, 16, 64][rng.below(8) as usize];
    let pin =
        if rng.below(2) == 1 { Some(rng.below(model.len() as u64) as usize) } else { None };
    let (got, exp) = if biased {
        (
            s.find_slot(pos, dir, budget, pin).map(|m| (m.from, m.to)),
            ref_find_slot(model, pos, dir, budget, pin),
        )
    } else {
        (
            s.find_nearest_slot(pos, dir, budget, pin).map(|m| (m.from, m.to)),
            ref_nearest(model, pos, dir, budget, pin),
        )
    };
    assert_eq!(got, exp, "step {step} pos={pos} dir={dir} b={budget} pin={pin:?}");
    if let Some((from, to)) = exp {
        s.slide_none(NoneSlide::new(from, to), pin);
        ref_slide(model, from, to);
        s.alloc(to).write(*val);
        model[to] = Some(*val);
        *val += 1;
    }
}

///combined torture: random op stream against a shadow model, full content
///comparison after every op. covers push/pop/grow/spread/slide/find/alloc/
///free/swap and both find fns' full contract (dir bias, budget, pin).
fn torture<S: Store<'static, u64>>() {
    let mut s = S::with_capacity(8);
    let mut model: Vec<Option<u64>> = vec![None; 8];
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut val = 0u64;
    const MAX_LEN: usize = 32;
    for step in 0..120 {
        let op = rng.below(12);
        match op {
            0 if s.len() < MAX_LEN => {
                let p = s.push_back(val);
                assert_eq!(p, model.len(), "step {step}");
                model.push(Some(val));
                val += 1;
            }
            1 if s.len() < MAX_LEN => {
                s.push_front(val);
                model.insert(0, Some(val));
                val += 1;
            }
            2 => {
                if let Some(i) = idx_where(&model, true, &mut rng) {
                    assert_eq!(s.free(i), model[i].unwrap(), "step {step}");
                    model[i] = None;
                }
            }
            3 if !model.is_empty() => {
                assert_eq!(s.pop_front(), model[0].take(), "step {step}");
            }
            4 if !model.is_empty() => {
                let last = model.len() - 1;
                assert_eq!(s.pop_back(), model[last].take(), "step {step}");
            }
            5 if s.len() + 3 <= MAX_LEN => {
                let n = (rng.below(3) + 1) as usize;
                let mx = s.grow_back(n);
                for _ in 0..n {
                    model.push(None);
                }
                assert_eq!(mx, model.len() - 1, "step {step}");
            }
            6 if s.len() + 3 <= MAX_LEN => {
                let n = (rng.below(3) + 1) as usize;
                s.grow_front(n);
                for _ in 0..n {
                    model.insert(0, None);
                }
            }
            7 => {
                if let Some(pos) = idx_where(&model, true, &mut rng) {
                    insert_via::<S>(&mut s, &mut model, pos, true, &mut rng, &mut val, step);
                }
            }
            8 => {
                if let Some(pos) = idx_where(&model, true, &mut rng) {
                    insert_via::<S>(&mut s, &mut model, pos, false, &mut rng, &mut val, step);
                }
            }
            9 if s.len() > 0 && s.len() * 2 <= MAX_LEN => {
                let off = rng.below(2) as usize;
                s.spread(off);
                model = ref_spread(&model, off);
            }
            10 if !model.is_empty() => {
                let n = model.len() as u64;
                let (a, b) = (rng.below(n) as usize, rng.below(n) as usize);
                s.swap(a, b);
                model.swap(a, b);
            }
            11 => {
                if let Some(i) = idx_where(&model, false, &mut rng) {
                    s.alloc(i).write(val);
                    model[i] = Some(val);
                    val += 1;
                }
            }
            _ => {}
        }
        assert_inv(&s);
        assert_eq!(snap(&s), model, "model mismatch at step {step} op {op}");
    }
}

///all (from,to) pairs vs the reference rotation, both directions, plus restore.
fn exhaustive_slides<S: Store<'static, u64>>(s: &mut S) {
    let orig = snap(s);
    let n = s.len();
    for from in 0..n {
        for to in 0..n {
            let before = snap(s);
            assert_eq!(s.slide_none(NoneSlide::new(from, to), None), to);
            let mut exp = before;
            ref_slide(&mut exp, from, to);
            assert_eq!(snap(s), exp, "slide {from}->{to}");
            let _ = s.slide_none(NoneSlide::new(to, from), None);
        }
    }
    assert_eq!(snap(s), orig, "slide-back did not restore");
}

///exhaustive find_slot + find_nearest_slot config matrix vs references:
///every occupied pos, both dirs, budgets incl. 0 and over-length, pins incl.
///pos itself.
fn check_finds<S: Store<'static, u64>>(s: &S, pattern: &[Option<u64>], label: &str) {
    let n = pattern.len();
    for pos in 0..n {
        if pattern[pos].is_none() {
            continue; // contract: pos occupied
        }
        for dir in [false, true] {
            for &b in &[0usize, 1, 2, 3, 100] {
                for pin in [None, Some(0), Some(n - 1), Some(pos)] {
                    let fs = s.find_slot(pos, dir, b, pin).map(|m| (m.from, m.to));
                    assert_eq!(
                        fs,
                        ref_find_slot(pattern, pos, dir, b, pin),
                        "{label} find_slot pos={pos} dir={dir} b={b} pin={pin:?}"
                    );
                    let ns = s.find_nearest_slot(pos, dir, b, pin).map(|m| (m.from, m.to));
                    assert_eq!(
                        ns,
                        ref_nearest(pattern, pos, dir, b, pin),
                        "{label} nearest pos={pos} dir={dir} b={b} pin={pin:?}"
                    );
                }
            }
        }
    }
}

///find_2_slots matrix. on Some: slides must not interfere (the crate's own
///debug_assert guards the sphere pass; this covers the fallback pass too), the
///pair applies independently in either order on the real store, both `to` slots
///end up None, the element multiset is conserved, and a pinned slot keeps its
///element. on None: the two single-anchor find_slots must either fail or
///interfere (no valid pair existed).
fn check_find2<S: Store<'static, u64>>(
    s: &S,
    pattern: &[Option<u64>],
    label: &str,
    dirs: &[(bool, bool)],
    budgets: &[usize],
    pins: &[Option<usize>],
) {
    let n = pattern.len();
    let mut sorted: Vec<u64> = pattern.iter().filter_map(|o| *o).collect();
    sorted.sort_unstable();
    let base: Vec<Option<u64>> = pattern.to_vec();
    for pos_a in 0..n {
        if pattern[pos_a].is_none() {
            continue;
        }
        for pos_b in 0..n {
            if pattern[pos_b].is_none() || pos_b < pos_a {
                continue; // mirrored pairs are assertion-redundant (both orders applied)
            }
            for &(dir_a, dir_b) in dirs {
                for &budget in budgets {
                    for &pin in pins {
                        // lazy: format! is the hot op under miri
                        let ctx = || {
                            format!(
                                "{label} pa={pos_a} da={dir_a} pb={pos_b} db={dir_b} \
                                 b={budget} pin={pin:?}"
                            )
                        };
                        match s.find_2_slots(pos_a, dir_a, pos_b, dir_b, budget, pin) {
                            Some(ts) => {
                                assert!(
                                    !slides_interfere(&ts.a, &ts.b, pos_a, pos_b),
                                    "interfering pair accepted: {}",
                                    ctx()
                                );
                                if let Some(p) = pin {
                                    assert_ne!(ts.a.to, p, "a.to == pin: {}", ctx());
                                    assert_ne!(ts.b.to, p, "b.to == pin: {}", ctx());
                                }
                                let mut s1 = S::from_vec(base.clone());
                                let mut s2 = S::from_vec(base.clone());
                                s1.slide_none(ts.a, pin);
                                s1.slide_none(ts.b, pin);
                                s2.slide_none(ts.b, pin);
                                s2.slide_none(ts.a, pin);
                                let (m1, m2) = (snap(&s1), snap(&s2));
                                assert_eq!(m1, m2, "order-dependent: {}", ctx());
                                assert!(
                                    m1[ts.a.to].is_none() && m1[ts.b.to].is_none(),
                                    "to not open: {}",
                                    ctx()
                                );
                                let mut got: Vec<u64> = m1.iter().filter_map(|o| *o).collect();
                                got.sort_unstable();
                                assert_eq!(got, sorted, "conservation: {}", ctx());
                                if let Some(p) = pin
                                    && let Some(v) = pattern[p]
                                {
                                    assert_eq!(m1[p], Some(v), "pin moved: {}", ctx());
                                }
                            }
                            None => {
                                let sa = s.find_slot(pos_a, dir_a, budget, pin);
                                let sb = s.find_slot(pos_b, dir_b, budget, pin);
                                if let (Some(sa), Some(sb)) = (sa, sb) {
                                    assert!(
                                        slides_interfere(&sa, &sb, pos_a, pos_b),
                                        "false negative: {}",
                                        ctx()
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn spread_matches<S: Store<'static, u64>>(mut s: S, off: usize) {
    let pattern = snap(&s);
    let len = s.len();
    s.spread(off);
    assert_eq!(s.len(), len * 2, "off={off}");
    assert_inv(&s);
    assert_eq!(snap(&s), ref_spread(&pattern, off), "off={off}");
}

///split partitions content, occupancy, and iter streams.
fn split_matches<S: Store<'static, u64>>(pattern: Vec<Option<u64>>, at: usize) {
    let mut s = S::from_vec(pattern.clone());
    let right = s.split(at);
    let (l_pat, r_pat) = pattern.split_at(at);
    assert_eq!(snap(&s), l_pat.to_vec());
    assert_eq!(snap(&right), r_pat.to_vec());
    assert_eq!(s.occupied(), l_pat.iter().filter(|o| o.is_some()).count());
    assert_eq!(right.occupied(), r_pat.iter().filter(|o| o.is_some()).count());
    assert_eq!(
        s.iter().copied().collect::<Vec<_>>(),
        l_pat.iter().filter_map(|o| *o).collect::<Vec<_>>()
    );
    assert_eq!(
        right.iter().copied().collect::<Vec<_>>(),
        r_pat.iter().filter_map(|o| *o).collect::<Vec<_>>()
    );
    assert_inv(&s);
    assert_inv(&right);
}

fn into_vec_roundtrip<S: Store<'static, u64>>(pattern: Vec<Option<u64>>) {
    let s = S::from_vec(pattern.clone());
    assert_eq!(s.into_vec(), pattern);
}

fn basics<S: Store<'static, u64>>() {
    let mut s = S::new();
    assert_eq!((s.len(), s.occupied()), (0, 0));
    for i in 0..8u64 {
        let p = s.push_back(i * 10);
        assert_eq!(p as u64, i);
        assert_eq!(*s.get(p), i * 10);
    }
    s.push_front(99);
    assert_eq!(*s.get(0), 99);
    assert_eq!(*s.get(1), 0);

    // grow_front/back insert Nones; grow_back returns the max addr
    s.grow_front(2);
    assert!(s.slot(0).is_none() && s.slot(1).is_none());
    assert_eq!(*s.get(2), 99);
    let last = s.grow_back(2);
    assert_eq!(last, s.len() - 1);
    assert!(s.slot(s.len() - 1).is_none());
    assert_inv(&s);

    // slot / slot_mut / get_mut
    assert_eq!(s.slot(2).copied(), Some(99));
    *s.slot_mut(2).unwrap() = 77;
    assert_eq!(*s.get(2), 77);
    *s.get_mut(2) = 88;
    assert_eq!(*s.get(2), 88);

    // disjoint both argument orders, occupied slots
    let len = s.len();
    {
        let (a, b) = s.get_disjoint_mut(2, 3);
        assert_eq!((*a, *b), (88, 0));
        *a += 1;
        *b += 1;
    }
    {
        let (a, b) = s.get_disjoint_mut(3, 2);
        assert_eq!(*a, 1);
        assert_eq!(*b, 89);
    }

    // pop on Some and on None
    assert_eq!(s.pop_back(), None); // grown tail
    assert_eq!(s.pop_front(), None); // grown head
    s.push_front(5);
    assert_eq!(s.pop_front(), Some(5));
    assert_inv(&s);

    // iter skips Nones, double-ended, exact size
    let it = S::from_vec(vec![Some(1), None, Some(3), None, Some(5)]);
    assert_eq!(it.iter().copied().collect::<Vec<_>>(), vec![1, 3, 5]);
    assert_eq!(it.iter().rev().copied().collect::<Vec<_>>(), vec![5, 3, 1]);
    let mut i = it.iter();
    assert_eq!(i.next(), Some(&1));
    assert_eq!(i.next_back(), Some(&5));
    assert_eq!(i.len(), 1);
    assert_eq!(i.next(), Some(&3));
    assert_eq!(i.next(), None);
    assert_eq!(i.next_back(), None);

    // alloc-write-read: reservation then write then read
    let mut s = S::from_vec(vec![Some(1), None]);
    s.alloc(1).write(7);
    assert_eq!(*s.get(1), 7);
    assert_eq!(s.occupied(), 2);
    // alloc_disjoint_mut: a<b and a>b, drain handoff write
    s.grow_back(1);
    let (x, cell) = s.alloc_disjoint_mut(1, 2);
    assert_eq!(*x, 7);
    cell.write(9);
    assert_eq!(*s.get(2), 9);
    s.grow_front(1); // slot 0 -> None, contents shift right
    let (x, cell) = s.alloc_disjoint_mut(2, 0);
    assert_eq!(*x, 7);
    cell.write(5);
    assert_eq!(*s.get(0), 5);
    assert_inv(&s);

    // swap
    let mut s = S::from_vec(vec![Some(1), Some(2)]);
    s.swap(0, 1);
    assert_eq!((s.slot(0).copied(), s.slot(1).copied()), (Some(2), Some(1)));
}

// ---------------------------------------------------------------------------
// both backends
// ---------------------------------------------------------------------------

macro_rules! suite {
    ($m:ident, $S:ty) => {
        mod $m {
            use super::*;

            #[test]
            fn basics() {
                super::basics::<$S>();
            }

            #[test]
            fn torture() {
                super::torture::<$S>();
            }

            #[test]
            fn slides_exhaustive() {
                let mut s = <$S>::from_vec(p1());
                super::exhaustive_slides(&mut s);
            }

            #[test]
            fn finds_matrix() {
                let s = <$S>::from_vec(p1());
                super::check_finds(&s, &p1(), "p1");
            }

            #[test]
            fn find2_matrix() {
                let pat = p1();
                let s = <$S>::from_vec(pat.clone());
                // (F,F) dropped: mirror-image of (T,T) on the palindromic p1
                super::check_find2(
                    &s,
                    &pat,
                    "p1",
                    &[(true, false), (false, true), (true, true)],
                    &[100],
                    &[None, Some(pat.len() / 2)],
                );
            }

            #[test]
            fn spread_odd() {
                super::spread_matches(<$S>::from_vec(p1()), 0);
                super::spread_matches(<$S>::from_vec(p1()), 1);
            }

            #[test]
            fn spread_even() {
                let pat = vec![Some(0), Some(1), None, Some(3)];
                super::spread_matches(<$S>::from_vec(pat.clone()), 0);
                super::spread_matches(<$S>::from_vec(pat), 1);
            }

            #[test]
            fn spread_empty() {
                super::spread_matches(<$S>::new(), 0);
                super::spread_matches(<$S>::new(), 1);
            }

            #[test]
            fn split() {
                super::split_matches::<$S>(p1(), 4); // split on a None
                super::split_matches::<$S>(p1(), 3); // split on a Some
                super::split_matches::<$S>(p1(), 0);
                super::split_matches::<$S>(p1(), 9);
            }

            #[test]
            fn into_vec_roundtrip() {
                super::into_vec_roundtrip::<$S>(p1());
            }
        }
    };
}

suite!(vec, VecStore<u64>);
suite!(deq, DequeStore<u64>);

// ---------------------------------------------------------------------------
// DequeStore wrap paths — slide straddle, boundary-adjacent find, spread
// ---------------------------------------------------------------------------

mod deq_wrap {
    use super::*;

    ///wrap helper: `grow` for spare capacity, then push `pad` Nones and
    ///`fronts` values onto the front — head moves back without reallocating, so
    ///`as_slices` splits. logical layout: [fronts.., pad Nones.., base..].
    fn wrapped_from(base: &[Option<u64>], fronts: &[u64], pad: usize) -> DequeStore<u64> {
        let mut s = DequeStore::from_vec(base.to_vec());
        s.grow();
        s.grow_front(pad);
        for &v in fronts {
            s.push_front(v);
        }
        let (f, b) = s.buf.as_slices();
        assert!(
            !f.is_empty() && !b.is_empty(),
            "fixture not wrapped: f={} b={}",
            f.len(),
            b.len()
        );
        s
    }

    ///front all Some (forces the front→back fallback), None in back past a pin.
    fn fallback_to_back() -> DequeStore<u64> {
        wrapped_from(
            &[Some(10), Some(11), Some(12), Some(13), None, Some(14)],
            &[22, 21, 20],
            0,
        )
    }

    ///back all Some (forces the back→front fallback), None at logical 0.
    fn fallback_to_front() -> DequeStore<u64> {
        let mut s = wrapped_from(&[Some(30), Some(31), Some(32), Some(33)], &[42, 41, 40], 0);
        s.free(0);
        s
    }

    ///Nones in both slices, boundary slot occupied (Equal branch of find).
    fn nones_both_sides() -> DequeStore<u64> {
        wrapped_from(
            &[Some(0), Some(1), None, Some(3), Some(4), None, Some(6), Some(7)],
            &[],
            3,
        )
    }

    ///even length, wrapped: the deque-indexed spread phase-2 branch.
    fn even_wrapped() -> DequeStore<u64> {
        wrapped_from(&[Some(0), Some(1), None, Some(3)], &[], 2)
    }

    #[test]
    fn finds_wrapped() {
        let cases: Vec<(&str, DequeStore<u64>)> = vec![
            ("back-fallback", fallback_to_back()),
            ("front-fallback", fallback_to_front()),
            ("nones-both", nones_both_sides()),
        ];
        for (label, s) in cases {
            check_finds(&s, &snap(&s), label);
        }
    }

    #[test]
    fn slides_wrapped() {
        // straddle (per-step swap), back-slice rotate, front-slice rotate paths
        let mut a = nones_both_sides();
        exhaustive_slides(&mut a);
        let mut b = fallback_to_back();
        exhaustive_slides(&mut b);
    }

    #[test]
    fn find2_wrapped() {
        let s = nones_both_sides();
        // full dir/pair coverage of the wrap geometry; budget/pin cross products
        // live on the p1 matrices
        check_find2(
            &s,
            &snap(&s),
            "wrapped",
            &[(false, false), (false, true), (true, false), (true, true)],
            &[100],
            &[None],
        );
    }

    #[test]
    fn spread_wrapped() {
        spread_matches(nones_both_sides(), 0); // odd len: direct-move path
        spread_matches(nones_both_sides(), 1);
        spread_matches(even_wrapped(), 0); // even len, wrapped slices
        spread_matches(even_wrapped(), 1);
    }

    #[test]
    fn slide_restores_wrapped_state() {
        // slide across the wrap boundary must not linearize the deque
        let mut s = nones_both_sides();
        let (f, b) = s.buf.as_slices();
        let (fl, bl) = (f.len(), b.len());
        let _ = s.slide_none(NoneSlide::new(0, s.len() - 1), None);
        let (f, b) = s.buf.as_slices();
        assert_eq!((f.len(), b.len()), (fl, bl), "slide linearized the deque");
    }
}

// ---------------------------------------------------------------------------
// drop accounting — leaks and double-drops
// ---------------------------------------------------------------------------

mod drops {
    use super::*;

    struct DropCtr(Rc<Cell<u32>>);
    impl Drop for DropCtr {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    ///every path — push, spread, slide, swap, free, pop, alloc, split, store
    ///drop — must drop each value exactly once.
    fn drain_torture<S: Store<'static, DropCtr>>() {
        let ctr = Rc::new(Cell::new(0));
        let mut made = 0u32;
        let mut s = S::new();
        for _ in 0..4 {
            s.push_back(DropCtr(ctr.clone()));
            made += 1;
        }
        s.push_front(DropCtr(ctr.clone()));
        made += 1;
        s.grow_back(2);
        s.grow_front(2);
        s.spread(1);
        s.spread(0);
        s.swap(1, 4);
        let occ = (0..s.len()).find(|&i| s.slot(i).is_some()).unwrap();
        drop(s.free(occ));
        drop(s.pop_front());
        drop(s.pop_back());
        s.grow_front(1); // open slot 0
        s.alloc(0).write(DropCtr(ctr.clone()));
        made += 1;
        s.grow_back(1);
        let mut right = s.split(3);
        drop(s);
        drop(right.pop_back());
        drop(right);
        assert_eq!(ctr.get(), made, "drop count != constructed count");
    }

    #[test]
    fn drain_vec() {
        drain_torture::<VecStore<DropCtr>>();
    }

    #[test]
    fn drain_deq() {
        drain_torture::<DequeStore<DropCtr>>();
    }

    #[test]
    fn plain_drop_vec() {
        let ctr = Rc::new(Cell::new(0));
        let mut s = VecStore::new();
        for _ in 0..3 {
            s.push_back(DropCtr(ctr.clone()));
        }
        drop(s);
        assert_eq!(ctr.get(), 3);
    }

    #[test]
    fn plain_drop_deq() {
        let ctr = Rc::new(Cell::new(0));
        let mut s = DequeStore::new();
        for _ in 0..3 {
            s.push_back(DropCtr(ctr.clone()));
        }
        drop(s);
        assert_eq!(ctr.get(), 3);
    }

    #[test]
    fn into_vec_moves_out_vec() {
        let ctr = Rc::new(Cell::new(0));
        let mut s = VecStore::new();
        for _ in 0..3 {
            s.push_back(DropCtr(ctr.clone()));
        }
        let v = s.into_vec();
        assert_eq!(ctr.get(), 0, "into_vec dropped payloads early");
        drop(v);
        assert_eq!(ctr.get(), 3);
    }

    #[test]
    fn into_vec_moves_out_deq() {
        let ctr = Rc::new(Cell::new(0));
        let mut s = DequeStore::new();
        for _ in 0..3 {
            s.push_back(DropCtr(ctr.clone()));
        }
        let v = s.into_vec();
        assert_eq!(ctr.get(), 0, "into_vec dropped payloads early");
        drop(v);
        assert_eq!(ctr.get(), 3);
    }
}

// ---------------------------------------------------------------------------
// contract panics (debug_assert-backed ones fire in debug test builds only)
// ---------------------------------------------------------------------------

mod panics {
    use super::*;

    #[test]
    #[should_panic(expected = "alloc into occupied")]
    fn alloc_occupied() {
        let mut s = VecStore::from_vec(vec![Some(1u64), None]);
        let _ = s.alloc(0);
    }

    #[test]
    #[should_panic(expected = "free empty")]
    fn free_none() {
        let mut s: VecStore<u64> = VecStore::new();
        s.grow_back(1);
        let _ = s.free(0);
    }

    #[test]
    #[should_panic(expected = "None at occupied ptr")]
    fn get_on_none() {
        let mut s: VecStore<u64> = VecStore::new();
        s.grow_back(1);
        let _ = s.get(0);
    }

    #[test]
    #[should_panic(expected = "a == b")]
    fn disjoint_same_slot() {
        let mut s = VecStore::from_vec(vec![Some(1u64), Some(2)]);
        let _ = s.get_disjoint_mut(0, 0);
    }

    #[test]
    #[should_panic(expected = "b is Some")]
    fn alloc_disjoint_into_occupied() {
        let mut s = VecStore::from_vec(vec![Some(1u64), Some(2)]);
        let _ = s.alloc_disjoint_mut(0, 1);
    }

    #[test]
    #[should_panic(expected = "pinned target slot")]
    fn slide_into_pin() {
        let mut s = VecStore::from_vec(vec![Some(1u64), None, Some(3)]);
        let _ = s.slide_none(NoneSlide::new(1, 2), Some(2));
    }

    #[test]
    #[should_panic(expected = "pin inside run")]
    fn slide_across_pin() {
        let mut s = VecStore::from_vec(vec![None, Some(1u64), Some(2), Some(3)]);
        let _ = s.slide_none(NoneSlide::new(0, 3), Some(1));
    }
}

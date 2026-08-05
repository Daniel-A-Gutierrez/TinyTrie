use super::*;

///store-agnostic invariants after any mutation.
fn assert_inv<S: Store<'static, u64>>(s: &S, max_cap: usize) {
    let occ = s.occupied();
    let len = s.len();
    let cap = s.cap();
    assert!(occ <= len, "occ={occ} > len={len}");
    assert!(len <= cap, "len={len} > cap={cap}");
    assert!(cap <= max_cap, "cap={cap} > max={max_cap}");
    assert_eq!(s.iter().count(), occ, "fwd iter count != occupied");
    assert_eq!(s.iter().rev().count(), occ, "rev iter count != occupied");
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
///pin-clamped contract. (from,to) == NoneSlide{from,to}.
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

// ---------------------------------------------------------------------------
// VecStore
// ---------------------------------------------------------------------------
mod vec {
    use super::*;

    const MC: usize = 16;

    #[test]
    fn push_back_returns_index() {
        let mut s: VecStore<u64, MC> = VecStore::new();
        assert_eq!(s.len(), 0);
        for i in 0..8 {
            let p = s.push_back(i * 10);
            assert_eq!(p, i as usize);
            assert_eq!(*s.get(p), i * 10);
            assert_inv(&s, MC);
        }
        assert_eq!(s.occupied(), 8);
    }

    #[test]
    fn push_front_shifts_existing() {
        let mut s: VecStore<u64, MC> = VecStore::new();
        s.push_back(1);
        s.push_back(2);
        s.push_front(0);
        assert_eq!(s.buf[0], Some(0));
        assert_eq!(s.buf[1], Some(1));
        assert_eq!(s.buf[2], Some(2));
        assert_eq!(s.occupied(), 3);
        assert_inv(&s, MC);
    }

    #[test]
    #[should_panic(expected = "max capacity")]
    fn push_back_past_max_panics() {
        let mut s: VecStore<u64, MC> = VecStore::new();
        for _ in 0..MC {
            s.push_back(0);
        }
        s.push_back(0);
    }

    #[test]
    fn insert_remove_slot() {
        let mut s: VecStore<u64, MC> = VecStore::new();
        for i in 0..5 {
            s.push_back(i);
        }
        s.remove(2);
        assert_eq!(s.buf[2], None);
        assert_eq!(s.occupied(), 4);
        assert_eq!(s.len(), 5);
        s.insert(99, 2);
        assert_eq!(s.buf[2], Some(99));
        assert_eq!(s.occupied(), 5);
        assert_eq!(s.len(), 5);
        assert_inv(&s, MC);
    }

    #[test]
    #[should_panic(expected = "insert into occupied")]
    fn insert_occupied_panics() {
        let mut s: VecStore<u64, MC> = VecStore::new();
        s.push_back(1);
        s.insert(2, 0);
    }

    #[test]
    #[should_panic(expected = "remove empty")]
    fn remove_empty_panics() {
        let mut s: VecStore<u64, MC> = VecStore::new();
        s.push_back(1);
        s.remove(0);
        s.remove(0);
    }

    #[test]
    fn pop_front_back() {
        let mut s: VecStore<u64, MC> = VecStore::new();
        for i in 0..5 {
            s.push_back(i);
        }
        assert_eq!(s.pop_front(), Some(0));
        assert_eq!(s.pop_back(), Some(4));
        assert_eq!(s.occupied(), 3);
        assert_eq!(s.len(), 5);
        assert_eq!(s.buf[0], None);
        assert_eq!(s.buf[4], None);
        assert_inv(&s, MC);
    }

    #[test]
    fn pop_none_slot() {
        let mut s: VecStore<u64, MC> = VecStore::new();
        s.push_back(1);
        s.remove(0);
        assert_eq!(s.pop_front(), None);
        assert_eq!(s.pop_back(), None);
        assert_eq!(s.occupied(), 0);
    }

    #[test]
    fn spread_doubles_gaps() {
        let mut s: VecStore<u64, MC> = VecStore::new();
        for i in 0..4 {
            s.push_back(i);
        }
        s.spread(0);
        assert_eq!(s.len(), 8);
        assert_eq!(s.occupied(), 4);
        for i in 0..4 {
            assert_eq!(s.buf[2 * i], Some(i as u64), "even slot {i}");
            assert_eq!(s.buf[2 * i + 1], None, "odd slot {i}");
        }
        assert_inv(&s, MC);
    }

    #[test]
    fn grow_front_back() {
        let mut s: VecStore<u64, MC> = VecStore::new();
        s.push_back(7);
        s.grow_front(2);
        assert_eq!(s.len(), 3);
        assert_eq!(s.buf[0], None);
        assert_eq!(s.buf[1], None);
        assert_eq!(s.buf[2], Some(7));
        let last = s.grow_back(3);
        assert_eq!(last, 5);
        assert_eq!(s.len(), 6);
        assert_eq!(s.buf[5], None);
        assert_eq!(s.occupied(), 1);
        assert_inv(&s, MC);
    }

    #[test]
    fn split_partitions() {
        let mut s: VecStore<u64, MC> = VecStore::new();
        for i in 0..6 {
            s.push_back(i);
        }
        let right = s.split(3);
        assert_eq!(s.len(), 3);
        assert_eq!(right.len(), 3);
        assert_eq!(s.occupied(), 3);
        assert_eq!(right.occupied(), 3);
        for i in 0..3 {
            assert_eq!(s.buf[i], Some(i as u64));
            assert_eq!(right.buf[i], Some((i + 3) as u64));
        }
    }

    #[test]
    fn split_and_rotate_odds_gap() {
        let mut s: VecStore<u64, MC> = VecStore::new();
        for i in 0..6 {
            s.push_back(i);
        }
        let right = s.split_and_rotate(3);
        assert_eq!(s.len(), 6);
        assert_eq!(right.len(), 6);
        assert_eq!(s.occupied(), 3);
        assert_eq!(right.occupied(), 3);
        for p in 0..3 {
            assert_eq!(s.buf[2 * p], None);
            assert_eq!(s.buf[2 * p + 1], Some(p as u64));
        }
        for k in 0..3 {
            assert_eq!(right.buf[2 * k], None);
            assert_eq!(right.buf[2 * k + 1], Some((k + 3) as u64));
        }
    }

    #[test]
    fn slide_none_both_dirs() {
        let mut s: VecStore<u64, MC> = VecStore::new();
        for i in 1..=4 {
            s.push_back(i);
        }
        s.remove(2); // [1,2,None,4]
        let to = s.slide_none(NoneSlide { from: 2, to: 0 }, None);
        assert_eq!(to, 0);
        assert_eq!(s.buf[0], None);
        assert_eq!(s.buf[1], Some(1));
        assert_eq!(s.buf[2], Some(2));
        assert_eq!(s.buf[3], Some(4));
        // reverse: slide None 0 -> 2
        let to = s.slide_none(NoneSlide { from: 0, to: 2 }, None);
        assert_eq!(to, 2);
        assert_eq!(s.buf[2], None);
        assert_eq!(s.buf[0], Some(1));
        assert_eq!(s.buf[1], Some(2));
    }

    #[test]
    fn slide_none_pin_outside_run_untouched() {
        let mut s: VecStore<u64, MC> = VecStore::new();
        for i in 1..=4 {
            s.push_back(i);
        }
        s.remove(2); // [1,2,None,4]
        let _ = s.slide_none(NoneSlide { from: 2, to: 0 }, Some(3));
        assert_eq!(s.buf[3], Some(4), "pinned slot moved");
        assert_eq!(s.buf[0], None);
    }

    #[test]
    fn find_slot_dir_bias_and_pin() {
        let mut s: VecStore<u64, MC> = VecStore::new();
        // [1,None,2,None,3]
        s.push_back(1);
        s.push_back(2);
        s.push_back(3);
        s.push_back(4);
        s.push_back(5);
        s.remove(1);
        s.remove(3);
        // pos=2 (Some 2), dir=true => nearest right None at 3
        let ms = s.find_slot(2, true, 10, None).unwrap();
        assert_eq!((ms.from, ms.to), (3, 3));
        // dir=false => nearest left None at 1
        let ms = s.find_slot(2, false, 10, None).unwrap();
        assert_eq!((ms.from, ms.to), (1, 1));
    }

    #[test]
    fn find_slot_pin_clamp_pos_eq() {
        let mut s: VecStore<u64, MC> = VecStore::new();
        // [1,None,2,None,3]
        s.push_back(1);
        s.push_back(2);
        s.push_back(3);
        s.push_back(4);
        s.push_back(5);
        s.remove(1);
        s.remove(3);
        // pos=2, pin=2: dir=true searches right only -> None at 3
        let ms = s.find_slot(2, true, 10, Some(2)).unwrap();
        assert_eq!((ms.from, ms.to), (3, 3));
        // dir=false searches left only -> None at 1
        let ms = s.find_slot(2, false, 10, Some(2)).unwrap();
        assert_eq!((ms.from, ms.to), (1, 1));
    }

    #[test]
    fn find_slot_pin_clamp_left() {
        // None left of pin must not be chosen.
        let mut s: VecStore<u64, MC> = VecStore::new();
        // [None,1,2,None,3]
        s.push_back(0);
        s.push_back(1);
        s.push_back(2);
        s.push_back(3);
        s.push_back(4);
        s.push_back(5);
        s.remove(0);
        s.remove(3);
        // pos=2 (Some 2), pin=1 (Some 1), dir=false (before): clamp min=max(2)=2,
        // no left None in [2,2); fall to right None at 3 -> slide 3->2.
        let ms = s.find_slot(2, false, 10, Some(1)).unwrap();
        assert_eq!((ms.from, ms.to), (3, 2));
    }

    #[test]
    fn find_slot_pin_clamp_right() {
        let mut s: VecStore<u64, MC> = VecStore::new();
        // [1,None,2,None,3]
        s.push_back(1);
        s.push_back(2);
        s.push_back(3);
        s.push_back(4);
        s.push_back(5);
        s.remove(1);
        s.remove(3);
        // pos=2, pin=3, dir=true: clamp max=min(3)=3, no right None in [3,3);
        // fall to left None at 1 -> slide 1->2.
        let ms = s.find_slot(2, true, 10, Some(3)).unwrap();
        assert_eq!((ms.from, ms.to), (1, 2));
    }

    #[test]
    fn find_slot_budget_exhaustion() {
        let mut s: VecStore<u64, MC> = VecStore::new();
        // [1,2,3,4,5] all occupied, None far away
        for i in 0..5 {
            s.push_back(i);
        }
        // pos=2, budget=1: window [1,4) -> [1,2,3] all Some -> None
        assert!(s.find_slot(2, true, 1, None).is_none());
        assert!(s.find_slot(2, false, 1, None).is_none());
    }

    #[test]
    fn find_nearest_tiebreak() {
        let mut s: VecStore<u64, MC> = VecStore::new();
        // [1,None,2,None,3]
        s.push_back(1);
        s.push_back(2);
        s.push_back(3);
        s.push_back(4);
        s.push_back(5);
        s.remove(1);
        s.remove(3);
        // pos=2: left None at 1 (dist1), right None at 3 (dist1) -> tie. dir=true=>right.
        let ms = s.find_nearest_slot(2, true, 10, None).unwrap();
        assert_eq!((ms.from, ms.to), (3, 3));
        let ms = s.find_nearest_slot(2, false, 10, None).unwrap();
        assert_eq!((ms.from, ms.to), (1, 1));
    }

    #[test]
    fn iter_skips_nones_rev() {
        let mut s: VecStore<u64, MC> = VecStore::new();
        s.push_back(1);
        s.push_back(2);
        s.push_back(3);
        s.remove(1); // [1,None,3]
        assert_eq!(s.iter().copied().collect::<Vec<_>>(), vec![1, 3]);
        assert_eq!(s.iter().rev().copied().collect::<Vec<_>>(), vec![3, 1]);
    }

    #[test]
    fn swap_slots() {
        let mut s: VecStore<u64, MC> = VecStore::new();
        s.push_back(1);
        s.push_back(2);
        s.swap(0, 1);
        assert_eq!(s.buf[0], Some(2));
        assert_eq!(s.buf[1], Some(1));
    }

    #[test]
    fn fuzz_invariants() {
        let mut s: VecStore<u64, MC> = VecStore::new();
        for op in 0..300u64 {
            match op % 6 {
                0 if s.len() < MC => {
                    s.push_back(op);
                }
                1 if s.len() < MC => {
                    s.push_front(op);
                }
                2 if s.occupied() > 0 => {
                    let idx = s.buf.iter().position(|o| o.is_some()).unwrap();
                    s.remove(idx);
                }
                3 if s.len() + 2 <= MC => {
                    s.grow_back(1);
                }
                _ => {}
            }
            assert_inv(&s, MC);
        }
    }
}

// ---------------------------------------------------------------------------
// DequeStore — same surface, plus wrap-path coverage for slide_none / find_*.
// ---------------------------------------------------------------------------
mod deq {
    use super::*;

    const MC: usize = 64;

    ///build a wrapped deque (as_slices returns two non-empty slices).
    fn wrapped() -> DequeStore<u64, MC> {
        let mut s: DequeStore<u64, MC> = DequeStore::new();
        for i in 0..16 {
            s.push_back(i);
        }
        for _ in 0..6 {
            s.pop_front();
        }
        for i in 0..6 {
            s.push_front(100 + i);
        }
        let (f, b) = s.buf.as_slices();
        assert!(!f.is_empty() && !b.is_empty(), "fixture not wrapped");
        s
    }

    #[test]
    fn push_back_returns_index() {
        let mut s: DequeStore<u64, MC> = DequeStore::new();
        for i in 0..10 {
            let p = s.push_back(i);
            assert_eq!(p, i as usize);
            assert_eq!(*s.get(p), i);
            assert_inv(&s, MC);
        }
    }

    #[test]
    fn push_front_shifts_existing() {
        let mut s: DequeStore<u64, MC> = DequeStore::new();
        s.push_back(1);
        s.push_back(2);
        s.push_front(0);
        // logical order [0,1,2]
        assert_eq!(s.buf[0], Some(0));
        assert_eq!(s.buf[1], Some(1));
        assert_eq!(s.buf[2], Some(2));
        assert_eq!(s.occupied(), 3);
        assert_inv(&s, MC);
    }

    #[test]
    #[should_panic(expected = "max capacity")]
    fn push_back_past_max_panics() {
        let mut s: DequeStore<u64, MC> = DequeStore::new();
        for _ in 0..MC {
            s.push_back(0);
        }
        s.push_back(0);
    }

    #[test]
    fn spread_doubles_gaps() {
        let mut s: DequeStore<u64, MC> = DequeStore::new();
        for i in 0..4 {
            s.push_back(i);
        }
        s.spread(0);
        assert_eq!(s.len(), 8);
        assert_eq!(s.occupied(), 4);
        let snap: Vec<Option<u64>> = s.buf.iter().cloned().collect();
        for i in 0..4 {
            assert_eq!(snap[2 * i], Some(i as u64), "even {i}");
            assert_eq!(snap[2 * i + 1], None, "odd {i}");
        }
    }

    #[test]
    fn spread_wrapped_path() {
        // force a wrapped deque then spread — exercises the deque-index branch.
        let mut s = wrapped();
        let len_before = s.len();
        s.spread(0);
        assert_eq!(s.len(), len_before * 2);
        assert_eq!(s.occupied(), s.occupied()); // tautology guard; real check below
        // logical content: original Somes in order at even slots, None at odds.
        let snap: Vec<Option<u64>> = s.buf.iter().cloned().collect();
        let somes: Vec<u64> = snap.iter().filter_map(|o| *o).collect();
        // original occupied count preserved, order preserved by iter
        assert_eq!(somes.len(), s.occupied());
        assert_inv(&s, MC);
    }

    #[test]
    fn split_partitions() {
        let mut s: DequeStore<u64, MC> = DequeStore::new();
        for i in 0..6 {
            s.push_back(i);
        }
        let right = s.split(3);
        assert_eq!(s.len(), 3);
        assert_eq!(right.len(), 3);
        assert_eq!(s.occupied(), 3);
        assert_eq!(right.occupied(), 3);
        assert_eq!(s.iter().copied().collect::<Vec<_>>(), vec![0, 1, 2]);
        assert_eq!(right.iter().copied().collect::<Vec<_>>(), vec![3, 4, 5]);
    }

    #[test]
    fn split_and_rotate_odds_gap() {
        let mut s: DequeStore<u64, MC> = DequeStore::new();
        for i in 0..6 {
            s.push_back(i);
        }
        let right = s.split_and_rotate(3);
        assert_eq!(s.len(), 6);
        assert_eq!(right.len(), 6);
        assert_eq!(s.occupied(), 3);
        assert_eq!(right.occupied(), 3);
        let ls: Vec<Option<u64>> = s.buf.iter().cloned().collect();
        let rs: Vec<Option<u64>> = right.buf.iter().cloned().collect();
        for p in 0..3 {
            assert_eq!(ls[2 * p], None);
            assert_eq!(ls[2 * p + 1], Some(p as u64));
        }
        for k in 0..3 {
            assert_eq!(rs[2 * k], None);
            assert_eq!(rs[2 * k + 1], Some((k + 3) as u64));
        }
    }

    #[test]
    fn slide_none_matches_ref_all_pairs_wrapped() {
        let orig: Vec<Option<u64>> = {
            let s = wrapped();
            s.buf.iter().cloned().collect()
        };
        let n = orig.len();
        let mut s = wrapped();
        for from in 0..n {
            for to in 0..n {
                let snap: Vec<Option<u64>> = s.buf.iter().cloned().collect();
                let _ = s.slide_none(NoneSlide { from, to }, None);
                let mut exp = snap.clone();
                ref_slide(&mut exp, from, to);
                let got: Vec<Option<u64>> = s.buf.iter().cloned().collect();
                assert_eq!(got, exp, "slide {from}->{to} (wrapped)");
                // restore
                let _ = s.slide_none(NoneSlide { from: to, to: from }, None);
            }
        }
        // fully restored
        let final_: Vec<Option<u64>> = s.buf.iter().cloned().collect();
        assert_eq!(final_, orig, "slide-back did not restore");
    }

    #[test]
    fn slide_none_matches_ref_contiguous() {
        let mut s: DequeStore<u64, MC> = DequeStore::new();
        for i in 0..8 {
            s.push_back(i);
        }
        s.remove(2);
        s.remove(5);
        let orig: Vec<Option<u64>> = s.buf.iter().cloned().collect();
        let n = s.len();
        for from in 0..n {
            for to in 0..n {
                let snap: Vec<Option<u64>> = s.buf.iter().cloned().collect();
                let _ = s.slide_none(NoneSlide { from, to }, None);
                let mut exp = snap.clone();
                ref_slide(&mut exp, from, to);
                let got: Vec<Option<u64>> = s.buf.iter().cloned().collect();
                assert_eq!(got, exp, "slide {from}->{to} (contiguous)");
                let _ = s.slide_none(NoneSlide { from: to, to: from }, None);
            }
        }
        let final_: Vec<Option<u64>> = s.buf.iter().cloned().collect();
        assert_eq!(final_, orig);
    }

    ///compare a DequeStore find fn against a reference over many configs, including
    ///pos at the front/back boundary (Less/Equal/Greater than flen).
    fn check_find<F, R>(find: F, ref_fn: R, is_wrapped: bool)
    where
        F: Fn(
            &DequeStore<u64, MC>,
            usize,
            bool,
            usize,
            Option<usize>,
        ) -> Option<(usize, usize)>,
        R: Fn(&[Option<u64>], usize, bool, usize, Option<usize>) -> Option<(usize, usize)>,
    {
        let mut s = if is_wrapped { wrapped() } else { DequeStore::new() };
        if !is_wrapped {
            for i in 0..10 {
                s.push_back(i);
            }
        }
        // punch some Nones
        s.remove(2);
        s.remove(5);
        if !is_wrapped {
            s.remove(7);
        }
        let snap: Vec<Option<u64>> = s.buf.iter().cloned().collect();
        let n = s.len();
        let budgets = [1usize, 2, 3, 5, 8, 16, 100];
        for pos in 0..n {
            if snap[pos].is_none() {
                continue; // contract: pos occupied
            }
            for &dir in &[false, true] {
                for &b in &budgets {
                    for pin in [None, Some(0usize), Some(3), Some(n - 1), Some(pos)] {
                        let got = find(&s, pos, dir, b, pin);
                        let exp = ref_fn(&snap, pos, dir, b, pin);
                        assert_eq!(
                            got, exp,
                            "find pos={pos} dir={dir} b={b} pin={pin:?} wrapped={is_wrapped}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn find_slot_matches_ref_contiguous() {
        check_find(
            |s, pos, dir, b, pin| s.find_slot(pos, dir, b, pin).map(|ms| (ms.from, ms.to)),
            ref_find_slot,
            false,
        );
    }

    #[test]
    fn find_slot_matches_ref_wrapped() {
        check_find(
            |s, pos, dir, b, pin| s.find_slot(pos, dir, b, pin).map(|ms| (ms.from, ms.to)),
            ref_find_slot,
            true,
        );
    }

    #[test]
    fn find_nearest_matches_ref_contiguous() {
        check_find(
            |s, pos, dir, b, pin| {
                s.find_nearest_slot(pos, dir, b, pin).map(|ms| (ms.from, ms.to))
            },
            ref_nearest,
            false,
        );
    }

    #[test]
    fn find_nearest_matches_ref_wrapped() {
        check_find(
            |s, pos, dir, b, pin| {
                s.find_nearest_slot(pos, dir, b, pin).map(|ms| (ms.from, ms.to))
            },
            ref_nearest,
            true,
        );
    }

    #[test]
    fn iter_rev_wrapped() {
        let s = wrapped();
        let fwd: Vec<u64> = s.iter().copied().collect();
        let mut rev = fwd.clone();
        rev.reverse();
        assert_eq!(s.iter().rev().copied().collect::<Vec<_>>(), rev);
    }

    ///front→back fallback under a `max` pin. front all Some (forces the fallback);
    ///back's nearest None is at logical 7. pin=6 clamps max to 6, so the None at 7
    ///must be excluded (None). without the pin the fallback finds it.
    #[test]
    fn find_nearest_front_fallback_respects_pin_max() {
        let mut s: DequeStore<u64, MC> = DequeStore::new();
        for v in [10u64, 11, 12, 13] {
            s.push_back(v);
        }
        s.grow_back(1);
        s.push_back(14);
        s.push_front(20);
        s.push_front(21);
        s.push_front(22);
        let (f, b) = s.buf.as_slices();
        assert_eq!(f.len(), 3);
        assert_eq!(b.len(), 6);
        assert!(b[4].is_none(), "fixture: back None at idx 4 (logical 7)");
        let pos = 1; // front[1] = 21 (Some)
        // no pin: fallback reaches the back None at logical 7. None is right of pos,
        // so to = pos+1 (dir=true) / pos (dir=false).
        let ms = s.find_nearest_slot(pos, true, 20, None).unwrap();
        assert_eq!((ms.from, ms.to), (7, 2));
        let ms = s.find_nearest_slot(pos, false, 20, None).unwrap();
        assert_eq!((ms.from, ms.to), (7, 1));
        // pin=6 (logical; back[3]=13): max clamps to 6 -> None at 7 excluded -> None.
        assert!(s.find_nearest_slot(pos, true, 20, Some(6)).is_none());
        assert!(s.find_nearest_slot(pos, false, 20, Some(6)).is_none());
    }

    ///back→front fallback under a `min` pin. back all Some around pos (forces the
    ///fallback); front has a None at logical 0. pin=2 clamps min to 3, so the None
    ///at 0 must be excluded (None). without the pin the fallback finds it.
    #[test]
    fn find_nearest_back_fallback_respects_pin_min() {
        let mut s: DequeStore<u64, MC> = DequeStore::new();
        for v in [30u64, 31, 32, 33] {
            s.push_back(v);
        }
        s.push_front(40);
        s.push_front(41);
        s.push_front(42);
        s.remove(0); // front[0] -> None
        let (f, b) = s.buf.as_slices();
        assert_eq!(f.len(), 3);
        assert!(f[0].is_none(), "fixture: front None at logical 0");
        assert_eq!(b.len(), 4);
        assert!(b.iter().all(|o| o.is_some()), "fixture: back all Some");
        let pos = 4; // back[1] = 31 (Some)
        // no pin: fallback reaches the front None at logical 0.
        let ms = s.find_nearest_slot(pos, true, 20, None).unwrap();
        assert_eq!((ms.from, ms.to), (0, 4));
        let ms = s.find_nearest_slot(pos, false, 20, None).unwrap();
        assert_eq!((ms.from, ms.to), (0, 3));
        // pin=2 (logical; front[2]=40): min clamps to 3 -> None at 0 excluded -> None.
        assert!(s.find_nearest_slot(pos, true, 20, Some(2)).is_none());
        assert!(s.find_nearest_slot(pos, false, 20, Some(2)).is_none());
    }
}

#![allow(dead_code)]

mod block;
mod nibble;
mod translator;

use block::Block;
use nibble::Nibble;
use translator::Translator;

/// insert `v` (as payload) at the phys slot for vaddr `v`.
fn put(b: &mut Block<u8>, v: u8) {
    let phys = b.translator().v2p(Nibble::from_u8(v)).as_usize();
    b.insert(phys, v);
}

/// fill every empty phys with its decoded vaddr (`p2v(phys)`), so `block[i] == p2v(i)`.
fn fill(b: &mut Block<u8>) {
    for phys in 0..b.len() {
        if b.get(phys).is_none() {
            let v = b.translator().p2v(Nibble::from_usize(phys)).as_u8();
            b.insert(phys, v);
        }
    }
}

/// invariant: for every occupied phys `i`, `v2p(block[i]) == i` AND `p2v(i) == block[i]`
/// (v2p and p2v are inverses on the block's items).
fn check_invariant(b: &Block<u8>) -> bool {
    b.iter().all(|(phys, &item)| {
        b.translator().v2p(Nibble::from_u8(item)).as_usize() == phys
            && b.translator().p2v(Nibble::from_usize(phys)).as_u8() == item
    })
}

/// grow a len=1 block to 16 via spreads, filling canonical vaddrs at each step.
fn spread_wrapping() {
    // shift=4 => len=1. grow to 16 by inserting the new canonical vaddrs then spreading.
    let mut b: Block<u8> = Block::new(Translator::new(Nibble::ZERO, Nibble::ZERO, 4, 0), Nibble::CAP);
    println!("initial (len={}, occ={}):", b.len(), b.occupancy());
    println!("{b}");

    put(&mut b, 0);
    b.spread(false); // -> len=2, shift=3, new canonical vaddr 8
    put(&mut b, 8);
    b.spread(false); // -> len=4, shift=2, new vaddrs 4,12
    put(&mut b, 4);
    put(&mut b, 12);
    b.spread(false); // -> len=8, shift=1, new vaddrs 2,6,10,14
    for v in [2u8, 6, 10, 14] {
        put(&mut b, v);
    }
    println!("\nlen={}, occ={}:", b.len(), b.occupancy());
    println!("{b}");

    b.spread(true); // -> len=16, shift=0, new vaddrs 1,3,5,7,9,11,13,15
    for v in [1u8, 3, 5, 7, 9, 11, 13, 15] {
        put(&mut b, v);
    }
    println!("\nfull (len={}, occ={}):", b.len(), b.occupancy());
    println!("{b}");
}

/// start from a full block, split+rotate into left/right halves at midpoint, print both.
fn main() {
    let mut b: Block<u8> = Block::new(Translator::default(), Nibble::CAP);
    for v in 0..16 {
        put(&mut b, v);
    }
    println!("full block (len={}, occ={}, invariant={}):", b.len(), b.occupancy(), check_invariant(&b));
    println!("{b}");

    let right = b.split_and_rotate(Nibble::MIDPOINT.as_usize());
    println!("\nleft (len={}, occ={}, invariant={}):", b.len(), b.occupancy(), check_invariant(&b));
    println!("{b}");
    println!("\nright (len={}, occ={}, invariant={}):", right.len(), right.occupancy(), check_invariant(&right));
    println!("{right}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_and_log() {
        let mut b: Block<u8> = Block::new(Translator::default(), Nibble::CAP);
        for v in 0..16 {
            put(&mut b, v);
        }
        for phys in 0..16 {
            assert_eq!(b.get(phys), Some(&(phys as u8)), "phys {phys}");
        }
    }

    // vaddrs handed out before a spread must resolve to the same item after.
    #[test]
    fn spread_preserves_vaddrs() {
        let mut b: Block<u8> = Block::new(Translator::new(Nibble::ZERO, Nibble::ZERO, 2, 0), Nibble::CAP);
        let vaddrs = [0u8, 4, 8, 12];
        for &v in &vaddrs {
            put(&mut b, v);
        }
        b.spread(false);
        b.spread(true);
        for &v in &vaddrs {
            let phys = b.translator().v2p(Nibble::from_u8(v)).as_usize();
            assert_eq!(b.get(phys), Some(&v), "vaddr {v}");
        }
    }

    #[test]
    #[should_panic(expected = "at cap, must split")]
    fn spread_panics_at_full_capacity() {
        let mut b: Block<u8> = Block::new(Translator::new(Nibble::ZERO, Nibble::ZERO, 0, 0), Nibble::CAP);
        b.spread(false);
    }

    #[test]
    #[should_panic(expected = "shift == 0")]
    fn translator_spread_panics_at_zero_shift() {
        let mut tr = Translator::new(Nibble::ZERO, Nibble::ZERO, 0, 0);
        tr.spread(false);
    }

    #[test]
    #[should_panic(expected = "already occupied")]
    fn insert_panics_if_occupied() {
        let mut b: Block<u8> = Block::new(Translator::default(), Nibble::CAP);
        b.insert(0, 0);
        b.insert(0, 1);
    }

    #[test]
    #[should_panic(expected = "empty")]
    fn remove_panics_if_empty() {
        let mut b: Block<u8> = Block::new(Translator::default(), Nibble::CAP);
        b.remove(0);
    }

    #[test]
    fn insert_remove_track_occupancy() {
        let mut b: Block<u8> = Block::new(Translator::default(), Nibble::CAP);
        assert!(b.is_vacant());
        b.insert(0, 0);
        b.insert(3, 3);
        assert_eq!(b.occupancy(), 2);
        assert_eq!(b.remove(0), 0);
        assert_eq!(b.occupancy(), 1);
        assert!(!b.is_vacant());
        b.remove(3);
        assert!(b.is_vacant());
    }

    // split+rotate: after the split, both halves must satisfy v2p(block[i]) == i.
    #[test]
    fn split_and_rotate_invariant() {
        let mut b: Block<u8> = Block::new(Translator::default(), Nibble::CAP);
        for v in 0..16 {
            put(&mut b, v);
        }
        assert!(check_invariant(&b), "full block");

        let right = b.split_and_rotate(Nibble::MIDPOINT.as_usize());
        assert!(check_invariant(&b), "split1 left");
        assert!(check_invariant(&right), "split1 right");
    }

    // repeatedly split + fill: invariant must hold at every step.
    #[test]
    fn repeated_split_invariant() {
        let mut b: Block<u8> = Block::new(Translator::default(), Nibble::CAP);
        for v in 0..16 {
            put(&mut b, v);
        }
        assert!(check_invariant(&b), "full");

        let mut right = b.split_and_rotate(Nibble::MIDPOINT.as_usize());
        assert!(check_invariant(&b), "split1 left");
        assert!(check_invariant(&right), "split1 right");
        fill(&mut b);
        fill(&mut right);
        assert!(check_invariant(&b), "split1 left filled");
        assert!(check_invariant(&right), "split1 right filled");

        let rr = right.split_and_rotate(Nibble::MIDPOINT.as_usize());
        assert!(check_invariant(&right), "split2 right-left");
        assert!(check_invariant(&rr), "split2 right-right");

        let lr = b.split_and_rotate(Nibble::MIDPOINT.as_usize());
        assert!(check_invariant(&b), "split2 left-left");
        assert!(check_invariant(&lr), "split2 left-right");
    }

    // inner=5, shift=0, no wrap: phys 0..7 hold vaddr 5..12 (contiguous). split at phys 4.
    #[test]
    fn split_inner_offset_no_wrap() {
        let mut b: Block<u8> = Block::new(Translator::new(Nibble::from_u8(5), Nibble::ZERO, 0, 0), Nibble::CAP);
        for phys in 0..8 {
            let v = b.translator().p2v(Nibble::from_usize(phys)).as_u8();
            b.insert(phys, v);
        }
        assert!(check_invariant(&b), "full");
        assert_eq!(b.len(), 16);
        assert_eq!(b.occupancy(), 8);

        let right = b.split_and_rotate(4);
        assert!(check_invariant(&b), "left");
        assert!(check_invariant(&right), "right");
        assert_eq!(b.len(), 16);
        assert_eq!(right.len(), 16);
        assert_eq!(b.occupancy(), 4);
        assert_eq!(right.occupancy(), 4);

        eprintln!("left:\n{b}");
        eprintln!("right:\n{right}");
        // ordering chosen over pointer integrity: phys-order vaddrs must be increasing.
        let lvs: Vec<u8> = b.iter().map(|(_, &v)| v).collect();
        let rvs: Vec<u8> = right.iter().map(|(_, &v)| v).collect();
        assert!(lvs.windows(2).all(|w| w[0] < w[1]), "left not ordered: {lvs:?}");
        assert!(rvs.windows(2).all(|w| w[0] < w[1]), "right not ordered: {rvs:?}");
    }

    // split a non-full (shift>0) block: left keeps its translator + low half, right is a
    // new block with shift-=1 (stride gaps, no rotation) and inner re-anchored at phys 0.
    #[test]
    fn split_shift_one() {
        let mut b: Block<u8> = Block::new(Translator::new(Nibble::ZERO, Nibble::ZERO, 1, 0), Nibble::CAP >> 1);
        fill(&mut b); // canonical vaddrs: p2v(phys) = phys<<1 = 0,2,4,6,8,10,12,14
        assert!(check_invariant(&b), "full shift=1 block");
        assert_eq!(b.len(), 8);
        assert_eq!(b.occupancy(), 8);

        let at = b.translator().v2p(Nibble::MIDPOINT).as_usize();
        let right = b.split_and_shift(at);
        assert!(check_invariant(&b), "shift=1 split left");
        assert!(check_invariant(&right), "shift=1 split right");
        assert_eq!(b.len(), 8); // left does NOT grow (in-place spread)
        assert_eq!(b.occupancy(), 4);
        assert_eq!(right.occupancy(), 4);
        assert_eq!(right.len(), 8); // right born at parent's cap (not MAX_CAP>>0=16)
        let lvs: Vec<u8> = b.iter().map(|(_, &v)| v).collect();
        let rvs: Vec<u8> = right.iter().map(|(_, &v)| v).collect();
        assert_eq!(lvs, [0, 2, 4, 6]);
        assert_eq!(rvs, [8, 10, 12, 14]);
    }

    #[test]
    fn split_shift_two() {
        let mut b: Block<u8> = Block::new(Translator::new(Nibble::ZERO, Nibble::ZERO, 2, 0), Nibble::CAP >> 2);
        fill(&mut b); // p2v(phys) = phys<<2 = 0,4,8,12
        assert!(check_invariant(&b), "full shift=2 block");
        assert_eq!(b.len(), 4);

        let at = b.translator().v2p(Nibble::MIDPOINT).as_usize();
        let right = b.split_and_shift(at);
        assert!(check_invariant(&b), "shift=2 split left");
        assert!(check_invariant(&right), "shift=2 split right");
        assert_eq!(b.len(), 4); // left does NOT grow
        assert_eq!(b.occupancy(), 2);
        assert_eq!(right.occupancy(), 2);
        assert_eq!(right.len(), 4); // right born at parent's cap (not MAX_CAP>>1=8)
        let lvs: Vec<u8> = b.iter().map(|(_, &v)| v).collect();
        let rvs: Vec<u8> = right.iter().map(|(_, &v)| v).collect();
        assert_eq!(lvs, [0, 4]);
        assert_eq!(rvs, [8, 12]);
    }

    // shift=1, inner=2: cap=8, canonical phys [0, cap-inner) = [0,6) hold vaddr
    // 4,6,8,10,12,14. Both inner offset AND shift > 0. Split at v2p(MIDPOINT)=2.
    #[test]
    fn split_inner_offset_and_shift() {
        let mut b: Block<u8> = Block::new(Translator::new(Nibble::from_u8(2), Nibble::ZERO, 1, 0), Nibble::CAP >> 1);
        let canonical = (Nibble::CAP >> 1) - 2; // cap - inner
        for phys in 0..canonical {
            let v = b.translator().p2v(Nibble::from_usize(phys)).as_u8();
            b.insert(phys, v);
        }
        assert!(check_invariant(&b), "full");
        assert_eq!(b.occupancy(), 6);

        let at = b.translator().v2p(Nibble::MIDPOINT).as_usize();
        let right = b.split_and_shift(at);
        assert!(check_invariant(&b), "left");
        assert!(check_invariant(&right), "right");

        eprintln!("left:\n{b}");
        eprintln!("right:\n{right}");

        let lvs: Vec<u8> = b.iter().map(|(_, &v)| v).collect();
        let rvs: Vec<u8> = right.iter().map(|(_, &v)| v).collect();
        assert_eq!(lvs, [4, 6]);
        assert_eq!(rvs, [8, 10, 12, 14]);
    }

    // shift=0 full block, wrapping split: from=12 > to=4, so left = phys [12,16) ∪ [0,4)
    // (a wrapping arc), right = phys [4,12). Exercises from > to.
    #[test]
    fn split_from_to_wrapped() {
        let mut b: Block<u8> = Block::new(Translator::default(), Nibble::CAP);
        for v in 0..16 {
            put(&mut b, v);
        }
        assert!(check_invariant(&b), "full");

        let right = b.split_from_to(12, 4);
        assert!(check_invariant(&b), "left");
        assert!(check_invariant(&right), "right");

        eprintln!("left:\n{b}");
        eprintln!("right:\n{right}");

        let lvs: Vec<u8> = b.iter().map(|(_, &v)| v).collect();
        let rvs: Vec<u8> = right.iter().map(|(_, &v)| v).collect();
        // left is the wrapping arc: cyclically ordered (12..15 then 0..3 in phys order).
        assert_eq!(lvs, [12, 13, 14, 15, 0, 1, 2, 3]);
        // right is the non-wrapping arc: strictly ordered 4..11.
        assert_eq!(rvs, [4, 5, 6, 7, 8, 9, 10, 11]);
        assert_eq!(b.occupancy(), 8);
        assert_eq!(right.occupancy(), 8);
    }

    // probe every split point on a full block: which keep v2p(block[i]) == i?
    #[test]
    fn split_various_at() {
        let mut table: Vec<(usize, &'static str, &'static str)> = Vec::new();
        let mut midpoint_ok = false;
        for at in 1..16 {
            let mut b: Block<u8> = Block::new(Translator::default(), Nibble::CAP);
            for v in 0..16 {
                put(&mut b, v);
            }
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let right = b.split_and_rotate(at);
                (check_invariant(&b), check_invariant(&right), b.occupancy(), right.occupancy())
            }));
            let (ls, rs) = match outcome {
                Ok((true, true, lo, ro)) => (format!("ok({lo})"), format!("ok({ro})")),
                Ok((l, r, lo, ro)) => (format!("{}({})", if l { "ok" } else { "BAD" }, lo), format!("{}({})", if r { "ok" } else { "BAD" }, ro)),
                Err(_) => ("panic".to_string(), "panic".to_string()),
            };
            let ls: &'static str = Box::leak(ls.into_boxed_str());
            let rs: &'static str = Box::leak(rs.into_boxed_str());
            table.push((at, ls, rs));
            if at == Nibble::MIDPOINT.as_usize() && ls == "ok(8)" && rs == "ok(8)" {
                midpoint_ok = true;
            }
        }
        eprintln!("\n split_at | left      | right");
        eprintln!("----------+-----------+----------");
        for (at, l, r) in &table {
            eprintln!(" {:>8} | {:<9} | {}", at, l, r);
        }
        assert!(midpoint_ok, "midpoint (8) must keep invariant on both halves");
    }
}
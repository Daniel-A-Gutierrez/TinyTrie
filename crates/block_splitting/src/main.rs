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

/// invariant: for every occupied phys `i`, `v2p(block[i]) == i`.
fn check_invariant(b: &Block<u8>) -> bool {
    b.iter().all(|(phys, &item)| {
        b.translator().v2p(Nibble::from_u8(item)).as_usize() == phys
    })
}

/// grow a len=1 block to 16 via spreads, filling canonical vaddrs at each step.
fn spread_wrapping() {
    // shift=4 => len=1. grow to 16 by inserting the new canonical vaddrs then spreading.
    let mut b: Block<u8> = Block::new(Translator::new(Nibble::ZERO, Nibble::ZERO, 4, 0));
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
    let mut b: Block<u8> = Block::new(Translator::default());
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
        let mut b: Block<u8> = Block::new(Translator::default());
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
        let mut b: Block<u8> = Block::new(Translator::new(Nibble::ZERO, Nibble::ZERO, 2, 0));
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
    #[should_panic(expected = "len 16 > 8")]
    fn spread_panics_at_full_capacity() {
        let mut b: Block<u8> = Block::new(Translator::new(Nibble::ZERO, Nibble::ZERO, 0, 0));
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
        let mut b: Block<u8> = Block::new(Translator::default());
        b.insert(0, 0);
        b.insert(0, 1);
    }

    #[test]
    #[should_panic(expected = "empty")]
    fn remove_panics_if_empty() {
        let mut b: Block<u8> = Block::new(Translator::default());
        b.remove(0);
    }

    #[test]
    fn insert_remove_track_occupancy() {
        let mut b: Block<u8> = Block::new(Translator::default());
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
        let mut b: Block<u8> = Block::new(Translator::default());
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
        let mut b: Block<u8> = Block::new(Translator::default());
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

    // split a non-full (shift>0) block at the vaddr midpoint: rotation is safe because
    // the split separates the {v, v+8} collision pairs across the two children.
    #[test]
    fn split_shift_one() {
        let mut b: Block<u8> = Block::new(Translator::new(Nibble::ZERO, Nibble::ZERO, 1, 0));
        fill(&mut b); // canonical vaddrs: p2v(phys) = phys<<1 = 0,2,4,6,8,10,12,14
        assert!(check_invariant(&b), "full shift=1 block");
        assert_eq!(b.len(), 8);
        assert_eq!(b.occupancy(), 8);

        let at = b.translator().v2p(Nibble::MIDPOINT).as_usize();
        let right = b.split_and_rotate(at);
        assert!(check_invariant(&b), "shift=1 split left");
        assert!(check_invariant(&right), "shift=1 split right");
        assert_eq!(b.occupancy(), 4);
        assert_eq!(right.occupancy(), 4);
    }

    #[test]
    fn split_shift_two() {
        let mut b: Block<u8> = Block::new(Translator::new(Nibble::ZERO, Nibble::ZERO, 2, 0));
        fill(&mut b); // p2v(phys) = phys<<2 = 0,4,8,12
        assert!(check_invariant(&b), "full shift=2 block");
        assert_eq!(b.len(), 4);

        let at = b.translator().v2p(Nibble::MIDPOINT).as_usize();
        let right = b.split_and_rotate(at);
        assert!(check_invariant(&b), "shift=2 split left");
        assert!(check_invariant(&right), "shift=2 split right");
        assert_eq!(b.occupancy(), 2);
        assert_eq!(right.occupancy(), 2);
    }

    // probe every split point on a full block: which keep v2p(block[i]) == i?
    #[test]
    fn split_various_at() {
        let mut table: Vec<(usize, &'static str, &'static str)> = Vec::new();
        let mut midpoint_ok = false;
        for at in 1..16 {
            let mut b: Block<u8> = Block::new(Translator::default());
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
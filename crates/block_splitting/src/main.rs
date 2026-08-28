#![allow(dead_code)]

mod block;
mod nibble;
mod translator;

use block::{Block, Node};
use nibble::Nibble;
use translator::Translator;

/// insert `v` (as `Node { v, val: v }`) at the phys slot for vaddr `v`. Delegates to
/// `Block::put` (auto-spreading); on already-full / shift==0 blocks it just inserts.
fn put(b: &mut Block<Node>, v: u8) {
    b.put(v);
}

/// pointer invariant: for every occupied phys `i`, the node's `v` satisfies
/// `v2p(v) == i` AND `p2v(i) == v` (v is the canonical vaddr for its phys).
fn check_invariant(b: &Block<Node>) -> bool {
    b.iter().all(|(phys, n)| {
        b.translator().v2p(Nibble::from_u8(n.v)).as_usize() == phys
            && b.translator().p2v(Nibble::from_usize(phys)).as_u8() == n.v
    })
}

/// ordering invariant: `val` strictly increasing in phys order (parent order).
fn ordered(b: &Block<Node>) -> bool {
    let vs: Vec<u8> = b.iter().map(|(_, n)| n.val).collect();
    vs.windows(2).all(|w| w[0] < w[1])
}

/// start from a full block, split+rotate into left/right halves at midpoint, print both.
fn main() {
    let mut b: Block<Node> = Block::new(Translator::default(), Nibble::CAP);
    for v in 0..16 {
        put(&mut b, v);
    }
    println!(
        "full block (len={}, occ={}, invariant={}):",
        b.len(),
        b.occupancy(),
        check_invariant(&b)
    );
    println!("{b}");

    let right = b.split(Nibble::MIDPOINT.as_usize());
    println!(
        "\nleft (len={}, occ={}, invariant={}):",
        b.len(),
        b.occupancy(),
        check_invariant(&b)
    );
    println!("{b}");
    println!(
        "\nright (len={}, occ={}, invariant={}):",
        right.len(),
        right.occupancy(),
        check_invariant(&right)
    );
    println!("{right}");
}

fn fw_print_arr<T>(v: &[T])
where T: std::fmt::Display {
    print!("[");
    for e in v {
        print!("{:3},", e);
    }
    print!("]\n");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordered_after_split() {
        // rotate-split path: full block, split at midpoint, both halves ordered.
        let mut b: Block<Node> = Block::uniform();
        for v in 0..16 { b.put(v); }
        assert!(ordered(&b), "full");
        let right = b.split(Nibble::MIDPOINT.as_usize());
        assert!(ordered(&b), "rotate left {b}");
        assert!(ordered(&right), "rotate right {right}");

        // shift-split path: shift=2 block, split at midpoint, both halves ordered.
        let mut b: Block<Node> = Block::new(Translator::new(Nibble::ZERO, Nibble::ZERO, 2, 0), Nibble::CAP >> 2);
        for phys in 0..b.len() {
            let v = b.translator().p2v(Nibble::from_usize(phys)).as_u8();
            b.insert(phys, Node { v, val: v });
        }
        assert!(ordered(&b), "shift full");
        let at = b.translator().v2p(Nibble::MIDPOINT).as_usize();
        let r2 = b.split(at);
        assert!(ordered(&b), "shift left {b}");
        assert!(ordered(&r2), "shift right {r2}");
    }

    /// off-midpoint (`at > mid`, left bigger): the choreography restores `val`-order on
    /// the left; the right (smaller) is ordered already. Both keep the pointer invariant.
    /// `e = at - mid < mid/2` is the feasibility bound (excess fits the top run).
    #[test]
    fn off_midpoint_left_fixup() {
        for &at in &[9usize, 10, 11] {
            let mut b: Block<Node> = Block::new(Translator::default(), Nibble::CAP);
            b.fill();
            assert!(check_invariant(&b) && ordered(&b), "full (at={at})");
            let right = b.split_and_rotate(at);
            assert!(ordered(&b), "left not ordered after off-midpoint fixup (at={at})\n{b}");
            assert!(check_invariant(&b), "left invariant broken (at={at})\n{b}");
            assert!(ordered(&right), "right not ordered (at={at})\n{right}");
            assert!(check_invariant(&right), "right invariant broken (at={at})\n{right}");
        }

        // infeasible excess (e == mid/2) must panic rather than corrupt the block.
        let mut b: Block<Node> = Block::new(Translator::default(), Nibble::CAP);
        b.fill();
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| { b.split_and_rotate(12); })).is_err());
    }

    /// off-midpoint (`at < mid`, right bigger): the choreography restores `val`-order on
    /// the right; the left (smaller) is ordered already. Both keep the pointer invariant.
    /// `e = mid - at < mid/2` is the feasibility bound. Not a clean mirror of the left case —
    /// the dense high-odd region compacts down to free the very-top phys for the excess.
    #[test]
    fn off_midpoint_right_fixup() {
        for &at in &[7usize, 6, 5] {
            let mut b: Block<Node> = Block::new(Translator::default(), Nibble::CAP);
            b.fill();
            assert!(check_invariant(&b) && ordered(&b), "full (at={at})");
            let right = b.split_and_rotate(at);
            assert!(ordered(&b), "left not ordered (at={at})\n{b}");
            assert!(check_invariant(&b), "left invariant broken (at={at})\n{b}");
            assert!(ordered(&right), "right not ordered after off-midpoint fixup (at={at})\n{right}");
            assert!(check_invariant(&right), "right invariant broken (at={at})\n{right}");
        }

        // infeasible excess (e == mid/2) must panic rather than corrupt the block.
        let mut b: Block<Node> = Block::new(Translator::default(), Nibble::CAP);
        b.fill();
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| { b.split_and_rotate(4); })).is_err());
    }

    #[test]
    fn rotate_range() {
        let mut io = 8;
        let mut count = 0;
        let translator = Translator::new(Nibble(io), Nibble::ZERO, 0, 1);
        let rt = Translator::new(Nibble((8 + io) % 16).rotate_left(1), Nibble::ZERO, 0, 1);
        let lt = Translator::new(Nibble(io).rotate_left(1), Nibble::ZERO, 0, 1);
        let mut virt = vec![];
        let mut phys = vec![];
        let mut p2vi = vec![];
        while count < 16 {
            virt.push((io + count) % 16);
            phys.push(rt.v2p(Nibble::from_u8((io + count) % 16)).as_usize());
            p2vi.push(rt.p2v(Nibble(count)).as_u8());
            count += 1;
        }

        println!("{:?}", rt);
        print!("before: ");
        fw_print_arr(&virt);
        print!("v2p(v): ");
        fw_print_arr(&phys); //should be 0..16
        print!("p2v(i): ");
        fw_print_arr(&p2vi); //should be v
    }

    /// Print probe for `compact_left`/`compact_right`: each carves a free run of `n`
    /// slots at one end by bubbling the nearest `n` frees into it, stable `None`↔`Some`
    /// swaps only. Asserts the carved region is fully free, order holds, and occupancy
    /// is unchanged.
    #[test]
    fn compact_demo() {
        // compact_left(4): carve [0,4) free. Frees live at 8,10,12,14; the four nearest
        // frees bubble left into [0,4), shifting the occupied slots in between right.
        let mut b: Block<Node> = Block::new(Translator::default(), Nibble::CAP);
        b.fill();
        for p in [8, 10, 12, 14] {
            b.remove(p);
        }
        let occ = b.occupancy();
        println!("\ncompact_left before: {b}");
        b.compact_left(4);
        println!("compact_left after:  {b}");
        assert!((0..4).all(|p| b.get(p).is_none()), "compact_left: [0,4) not all free");
        assert!(ordered(&b), "compact_left: order not preserved\n{b}");
        assert_eq!(b.occupancy(), occ, "compact_left: occupancy changed");

        // compact_right(4): carve [12,16) free. Frees live at 0,2,4,6; the four nearest
        // frees bubble right into [12,16), shifting the occupied slots in between left.
        let mut b: Block<Node> = Block::new(Translator::default(), Nibble::CAP);
        b.fill();
        for p in [0, 2, 4, 6] {
            b.remove(p);
        }
        let occ = b.occupancy();
        println!("\ncompact_right before: {b}");
        b.compact_right(4);
        println!("compact_right after: {b}");
        assert!((12..16).all(|p| b.get(p).is_none()), "compact_right: [12,16) not all free");
        assert!(ordered(&b), "compact_right: order not preserved\n{b}");
        assert_eq!(b.occupancy(), occ, "compact_right: occupancy changed");
    }

    /// Minimal-move + re-key: only the nearest `n` frees relocate; items past the last
    /// relocated free keep their `val` and phys, and the moved items keep their `val`
    /// (re-ordered) while `v` is re-keyed to `p2v(phys)`. Identity translator →
    /// `[0,x,2,x,4,x,6,x]`; phys 4 and phys 6 do not move (val 4, 6 stay), the trailing gap
    /// at phys 5 survives `compact_left(2)`, and the moved pair (phys 2,3) carry val 0, 2.
    #[test]
    fn compact_minimal_move() {
        let mut b: Block<Node> = Block::new(Translator::default(), 8);
        b.fill();
        for p in [1, 3, 5, 7] {
            b.remove(p);
        }
        b.compact_left(2);
        assert!(b.get(0).is_none() && b.get(1).is_none(), "phys 0,1 not free");
        assert_eq!(b.get(2).map(|n| n.val), Some(0), "phys 2 val");
        assert_eq!(b.get(3).map(|n| n.val), Some(2), "phys 3 val");
        assert_eq!(b.get(4).map(|n| n.val), Some(4), "phys 4 (unmoved)");
        assert!(b.get(5).is_none(), "phys 5 (trailing gap preserved)");
        assert_eq!(b.get(6).map(|n| n.val), Some(6), "phys 6 (unmoved)");
        assert!(b.get(7).is_none(), "phys 7");
        assert!(check_invariant(&b), "compact_left broke invariant\n{b}");

        // compact_right mirror: carve [6,8) free → trailing frees bubble in.
        let mut b: Block<Node> = Block::new(Translator::default(), 8);
        b.fill();
        for p in [1, 3, 5, 7] {
            b.remove(p);
        }
        b.compact_right(2);
        assert!(b.get(6).is_none() && b.get(7).is_none(), "phys 6,7 not free");
        assert_eq!(b.get(0).map(|n| n.val), Some(0), "phys 0 (unmoved)");
        assert!(b.get(1).is_none(), "phys 1 (leading gap preserved)");
        assert_eq!(b.get(2).map(|n| n.val), Some(2), "phys 2 (unmoved)");
        assert!(b.get(3).is_none(), "phys 3");
        assert_eq!(b.get(4).map(|n| n.val), Some(4), "phys 4 (unmoved)");
        assert_eq!(b.get(5).map(|n| n.val), Some(6), "phys 5 val");
        assert!(check_invariant(&b), "compact_right broke invariant\n{b}");
    }
}


/// Off-midpoint split across parent `inner` {0,4,8}, `rotation` 0..4, and all feasible `at`.
/// A full block is `val`-ordered for any translator (val = phys rank; vaddrs wrap freely), and
/// the choreography maintains `val`-order + the pointer invariant on both halves regardless of
/// the parent's rotation.
#[test]
fn off_midpoint_offsets() {
    for inner in [0u8, 4, 8] {
        for rot in 0u32..4 {
            for &at in &[5usize, 6, 7, 9, 10, 11] {
                let mut b: Block<Node> = Block::new(
                    Translator::new(Nibble::from_u8(inner), Nibble::ZERO, 0, rot),
                    Nibble::CAP,
                );
                b.fill();
                assert!(
                    ordered(&b) && check_invariant(&b),
                    "parent not ordered (inner={inner} rot={rot})"
                );
                let right = b.split_and_rotate(at);
                assert!(ordered(&b), "left not ordered (inner={inner} rot={rot} at={at})\n{b}");
                assert!(check_invariant(&b), "left invariant (inner={inner} rot={rot} at={at})\n{b}");
                assert!(ordered(&right), "right not ordered (inner={inner} rot={rot} at={at})\n{right}");
                assert!(
                    check_invariant(&right),
                    "right invariant (inner={inner} rot={rot} at={at})\n{right}"
                );
            }
        }
    }
}

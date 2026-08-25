#![allow(dead_code)]

mod block;
mod nibble;
mod translator;

use block::Block;
use nibble::Nibble;
use translator::Translator;

/// insert `v` (as payload) at the phys slot for vaddr `v`. Delegates to `Block::put`
/// (auto-spreading); on already-full / shift==0 blocks it just inserts (no spread).
fn put(b: &mut Block<u8>, v: u8) {
    b.put(v);
}

/// invariant: for every occupied phys `i`, `v2p(block[i]) == i` AND `p2v(i) == block[i]`
/// (v2p and p2v are inverses on the block's items).
fn check_invariant(b: &Block<u8>) -> bool {
    b.iter().all(|(phys, &item)| {
        b.translator().v2p(Nibble::from_u8(item)).as_usize() == phys
            && b.translator().p2v(Nibble::from_usize(phys)).as_u8() == item
    })
}

/// start from a full block, split+rotate into left/right halves at midpoint, print both.
fn main() {
    let mut b: Block<u8> = Block::new(Translator::default(), Nibble::CAP);
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

    /// phys-order vaddrs strictly increasing (the ordering invariant).
    fn ordered(b: &Block<u8>) -> bool {
        let vs: Vec<u8> = b.iter().map(|(_, &v)| v).collect();
        vs.windows(2).all(|w| w[0] < w[1])
    }

    #[test]
    fn ordered_after_split() {
        // rotate-split path: full block, split at midpoint, both halves ordered.
        let mut b: Block<u8> = Block::uniform();
        for v in 0..16 { b.put(v); }
        assert!(ordered(&b), "full");
        let mut right = b.split(Nibble::MIDPOINT.as_usize());
        assert!(ordered(&b), "rotate left {b}");
        assert!(ordered(&right), "rotate right {right}");

        // shift-split path: shift=2 block, split at midpoint, both halves ordered.
        let mut b: Block<u8> = Block::new(Translator::new(Nibble::ZERO, Nibble::ZERO, 2, 0), Nibble::CAP >> 2);
        for phys in 0..b.len() { b.insert(phys, b.translator().p2v(Nibble::from_usize(phys)).as_u8()); }
        assert!(ordered(&b), "shift full");
        let at = b.translator().v2p(Nibble::MIDPOINT).as_usize();
        let r2 = b.split(at);
        assert!(ordered(&b), "shift left {b}");
        assert!(ordered(&r2), "shift right {r2}");

        // off-midpoint rotate (expect ordering to BREAK — confirms the constraint).
        let mut b: Block<u8> = Block::uniform();
        for v in 0..16 { b.put(v); }
        let _r3 = b.split_and_rotate(5);
        eprintln!("off-midpoint rotate left: {b} (ordered={})", ordered(&b));
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
}

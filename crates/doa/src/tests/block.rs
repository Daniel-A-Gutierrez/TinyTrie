use super::*;
use crate::index::BlockIndex;
use crate::store::{DequeStore, Store, VecStore};
use crate::{InOrder, PreOrder};

///for every occupied slot, p2v(v2p(v))==v and vget(vaddr) matches the cursor.
fn roundtrip<B>(b: &B)
where
    B: BlockBase<'static, T = u64>,
    B::P: BlockIndex,
{
    let mut c = BlockCursor::new(b);
    c.first();
    while let Some(v) = c.address() {
        let p = b.v2p(v);
        assert_eq!(b.p2v(p), v, "p2v(v2p(v)) != v at vaddr {v:?}");
        assert_eq!(*b.vget(v), *c.current().unwrap(), "vget(v) != cursor at vaddr {v:?}");
        if c.next().is_none() {
            break;
        }
    }
}

///all recorded (vaddr,value) pairs still resolve.
fn stable<B>(b: &B, pairs: &[(B::P, u64)])
where
    B: BlockBase<'static, T = u64>,
    B::P: BlockIndex,
{
    for (v, val) in pairs {
        assert_eq!(*b.vget(*v), *val, "vaddr {:?} not stable", v);
    }
}

// ---------------------------------------------------------------------------
// FixedRoot<InOrder> (u16, VecStore) — root pinned at MIDPOINT
// ---------------------------------------------------------------------------
mod fixedroot {
    use super::*;
    type Blk = Block<'static, u64, u16, VecStore<u64, 4096>, FixedRoot<InOrder>>;

    #[test]
    fn new_empty() {
        let b: Blk = Blk::new();
        assert_eq!(b.len(), 0);
        assert_eq!(b.occupied(), 0);
        assert!(b.first_vaddr().is_none());
        assert_eq!(b.translator().shift(), 15);
        assert_eq!(b.translator().inner_offset(), 0);
    }

    #[test]
    fn insert_root_at_midpoint() {
        let mut b: Blk = Blk::new();
        let p = b.insert_root(42);
        let v = b.p2v(p);
        assert_eq!(v, 32768);
        assert_eq!(*b.vget(v), 42);
        assert_eq!(b.first_vaddr(), Some(v));
        assert_eq!(b.last_vaddr(), Some(v));
        roundtrip(&b);
    }

    #[test]
    fn mid_insert_after_root_preserves_root() {
        let mut b: Blk = Blk::new();
        let root = b.insert_root(100); let root = b.p2v(root);
        let ms = b.find_slot(b.v2p(root), true).slide.expect("slot");
        let slot = b.slide_none(ms);
        let new = b.insert(200, slot);
        assert_eq!(*b.vget(root), 100);
        assert_eq!(*b.get(new), 200);
        assert!(b.len().is_power_of_two());
        roundtrip(&b);
    }

    #[test]
    fn vaddr_stable_across_growth() {
        let mut b: Blk = Blk::new();
        let root = b.insert_root(0); let root = b.p2v(root);
        //insert after the last each time, pinning the root. spread/grow preserve all
        //vaddrs; slides preserve only the pin, so check root + roundtrip (v2p/p2v/vget
        //consistency) per step, not every recorded vaddr (displaced ones remap).
        for i in 1..40 {
            let last = b.last_vaddr().unwrap();
            let ms = b.find_slot(b.v2p(last), true).slide.expect("slot");
            let slot = b.slide_none(ms);
            b.insert(i, slot);
            assert_eq!(*b.vget(root), 0, "root moved at i={i}");
            roundtrip(&b);
        }
        assert!(b.len().is_power_of_two());
    }

    #[test]
    fn len_pow2_throughout() {
        let mut b: Blk = Blk::new();
        let _ = b.insert_root(0);
        for i in 1..20 {
            let last = b.last_vaddr().unwrap();
            let ms = b.find_slot(b.v2p(last), true).slide.expect("slot");
            let slot = b.slide_none(ms);
            b.insert(i, slot);
            assert!(b.len().is_power_of_two(), "len {} not pow2", b.len());
        }
    }

    //todo : fix
    // #[test]
    // fn remove_then_reuse() {
    //     let mut b: Blk = Blk::new();
    //     let root = b.insert_root(0); let root = b.p2v(root);
    //     let ms = b.find_slot(b.v2p(root), true).slide.expect("slot");
    //     let slot = b.slide_none(ms);
    //     let v = b.insert(1, slot);
    //     let removed = b.remove(v);
    //     assert_eq!(removed, 1);
    //     assert_eq!(b.occupied(), 1);
    //     // root still intact
    //     assert_eq!(*b.vget(root), 0);
    //     roundtrip(&b);
    // }

    type Small = Block<'static, u64, u16, VecStore<u64, 4>, FixedRoot<InOrder>>;

    #[test]
    fn exhaustion_returns_none() {
        let mut b: Small = Small::new();
        let root = b.insert_root(0); let root = b.p2v(root);
        // InOrder root sits at phys len/2. Fill around it with the pin keeping root
        // put: two before (leftward slides use phys1 then phys0) and one after (phys3),
        // reaching occupied=4=len=cap=max_capacity.
        let first = b.first_vaddr().unwrap();
        for i in 1..=2 {
            let ms = b.find_slot(b.v2p(first), false).slide.expect("slot before");
            let slot = b.slide_none(ms);
            b.insert(i, slot);
        }
        let last = b.last_vaddr().unwrap();
        let ms = b.find_slot(b.v2p(last), true).slide.expect("slot after");
        let slot = b.slide_none(ms);
        b.insert(3, slot);
        assert_eq!(b.occupied(), 4);
        assert_eq!(b.len(), b.max_capacity());
        let last = b.last_vaddr().unwrap();
        assert!(b.find_slot(b.v2p(last), true).slide.is_none(), "should be exhausted");
    }

    #[test]
    fn pin_root_never_moves() {
        let mut b: Blk = Blk::new();
        let root = b.insert_root(0); let root = b.p2v(root);
        for i in 1..30 {
            let last = b.last_vaddr().unwrap();
            let ms = b.find_slot(b.v2p(last), true).slide.expect("slot");
            let slot = b.slide_none(ms);
            b.insert(i, slot);
            // root vaddr stable + getable; InOrder root sits at phys len/2 (spread
            // doubles len so root phys moves 1->2->4...; the pin keeps slides off it).
            assert_eq!(*b.vget(root), 0);
            assert_eq!(b.v2p(root), b.len() / 2, "root not at len/2 at i={i}");
        }
    }
}

// ---------------------------------------------------------------------------
// Uniform (u16, VecStore) — no-pin full-range block (anchor 0)
// ---------------------------------------------------------------------------
mod uniform {
    use super::*;
    type Blk = Block<'static, u64, u16, VecStore<u64, 4096>, Uniform>;

    #[test]
    fn new_empty() {
        let b: Blk = Blk::new();
        assert_eq!(b.len(), 0);
        assert_eq!(b.occupied(), 0);
        assert!(b.first_vaddr().is_none());
        assert_eq!(b.translator().shift(), 16);
        assert_eq!(b.translator().inner_offset(), 0);
    }

    #[test]
    fn insert_root_at_zero() {
        let mut b: Blk = Blk::new();
        let p = b.insert_root(42);
        let v = b.p2v(p);
        assert_eq!(v, 0);
        assert_eq!(*b.vget(v), 42);
        roundtrip(&b);
    }

    ///no pin: the root is not special — it may move in a slide. we only check len pow2
    ///+ translator round-trip, not root stability.
    #[test]
    fn mid_insert_no_pin() {
        let mut b: Blk = Blk::new();
        let _ = b.insert_root(0);
        for i in 1..20 {
            let last = b.last_vaddr().unwrap();
            let ms = b.find_slot(b.v2p(last), true).slide.expect("slot");
            let slot = b.slide_none(ms);
            b.insert(i, slot);
            assert!(b.len().is_power_of_two(), "len {} not pow2", b.len());
            roundtrip(&b);
        }
    }
}

// ---------------------------------------------------------------------------
// Pluripotent (u16 + u32, DequeStore)
// ---------------------------------------------------------------------------
mod pluripotent {
    use super::*;
    type Blk16 = Block<'static, u64, u16, DequeStore<u64, 256>, Pluripotent>;
    type Blk32 = Block<'static, u64, u32, DequeStore<u64, 256>, Pluripotent>;

    #[test]
    fn new_empty_u16() {
        let b: Blk16 = Blk16::new();
        assert_eq!(b.translator().shift(), 7); // Half(u8)::BIT_WIDTH - 1
        assert_eq!(b.translator().inner_offset(), 0);
        assert_eq!(b.translator().outer_offset(), 32768);
    }

    #[test]
    fn new_empty_u32() {
        let b: Blk32 = Blk32::new();
        assert_eq!(b.translator().shift(), 15); // Half(u16)::BIT_WIDTH - 1
        assert_eq!(b.translator().inner_offset(), 0);
        assert_eq!(b.translator().outer_offset(), 1 << 31);
    }

    #[test]
    fn back_dense_stride() {
        let mut b: Blk16 = Blk16::new();
        let mut pairs = Vec::new();
        for i in 0..10 {
            let p = b.try_push_back(i).unwrap();
            pairs.push((b.p2v(p), i));
        }
        assert_eq!(pairs[0].0, 32768);
        assert_eq!(pairs[1].0, 32768 + (1 << 7)); // stride 128
        stable(&b, &pairs);
        roundtrip(&b);
    }

    #[test]
    fn back_dense_stride_u32() {
        let mut b: Blk32 = Blk32::new();
        let mut pairs = Vec::new();
        for i in 0..10 {
            let p = b.try_push_back(i).unwrap();
            pairs.push((b.p2v(p), i));
        }
        assert_eq!(pairs[0].0, 1 << 31);
        assert_eq!(pairs[1].0, (1 << 31) + (1 << 15));
        stable(&b, &pairs);
        roundtrip(&b);
    }

    ///regression: push_front must keep existing vaddrs stable for all addr_shift,
    ///not just 0. The offset bump is `1<<shift`, not `+1`.
    #[test]
    fn front_stable_across_push() {
        let mut b: Blk16 = Blk16::new();
        let back = b.try_push_back(10).unwrap(); let back = b.p2v(back);
        let front1 = b.try_push_front(20).unwrap(); let front1 = b.p2v(front1);
        assert_eq!(*b.vget(back), 10, "back vaddr moved after push_front");
        let front2 = b.try_push_front(30).unwrap(); let front2 = b.p2v(front2);
        assert_eq!(*b.vget(back), 10);
        assert_eq!(*b.vget(front1), 20, "front1 moved after second push_front");
        assert_eq!(*b.vget(front2), 30);
        roundtrip(&b);
    }

    #[test]
    fn front_stable_across_push_u32() {
        let mut b: Blk32 = Blk32::new();
        let back = b.try_push_back(10).unwrap(); let back = b.p2v(back);
        let front = b.try_push_front(20).unwrap(); let front = b.p2v(front);
        assert_eq!(*b.vget(back), 10);
        assert_eq!(*b.vget(front), 20);
        roundtrip(&b);
    }

    #[test]
    fn back_exhaustion() {
        let mut b: Blk16 = Blk16::new();
        for i in 0..256 {
            assert!(b.try_push_back(i).is_ok(), "failed at {i}");
        }
        assert!(b.try_push_back(999).is_err());
        assert!(b.len() <= b.max_capacity());
    }

    #[test]
    fn mid_insert_after_two_backs() {
        let mut b: Blk16 = Blk16::new();
        let r0 = b.try_push_back(0).unwrap(); let r0 = b.p2v(r0);
        let r1 = b.try_push_back(1).unwrap(); let r1 = b.p2v(r1);
        // mid-insert after r0 with r0 pinned
        let ms = b.find_slot(b.v2p(r0), true).slide.expect("slot");
        let slot = b.slide_none(ms);
        let v = b.insert(50, slot);
        assert_eq!(*b.vget(r0), 0);
        assert_eq!(*b.vget(r1), 1);
        assert_eq!(*b.get(v), 50);
        roundtrip(&b);
    }

    #[test]
    fn grow_and_spread_preserves_vaddr_even_len() {
        let mut b: Blk16 = Blk16::new();
        // start from len 2 (even) so spread's mid>0 path is exercised
        let a = b.try_push_back(1).unwrap(); let a = b.p2v(a);
        let _ = b.try_push_back(2).unwrap();
        b.grow_and_spread().ok().unwrap();
        assert_eq!(b.len(), 4);
        assert_eq!(*b.vget(a), 1, "vaddr not stable across spread");
        roundtrip(&b);
    }

    ///spread on len 1: element must remain at its vaddr, len becomes 2.
    #[test]
    fn grow_and_spread_len1() {
        let mut b: Blk16 = Blk16::new();
        let root = b.insert_root(7); let root = b.p2v(root);
        b.grow_and_spread().ok().unwrap();
        assert_eq!(b.len(), 2);
        assert_eq!(*b.vget(root), 7, "root not stable across spread(len1)");
        roundtrip(&b);
    }
}

// ---------------------------------------------------------------------------
// Append (u16, VecStore)
// ---------------------------------------------------------------------------
mod append {
    use super::*;
    type Blk = Block<'static, u64, u16, VecStore<u64, 512>, Append>;

    #[test]
    fn new_empty() {
        let b: Blk = Blk::new();
        assert_eq!(b.translator().shift(), 0);
        assert_eq!(b.translator().inner_offset(), 256);
    }

    #[test]
    fn back_dense_low_addrs() {
        let mut b: Blk = Blk::new();
        let mut pairs = Vec::new();
        for i in 0..20 {
            let p = b.try_push_back(i).unwrap();
            pairs.push((b.p2v(p), i));
        }
        assert_eq!(pairs[0].0, 256); // p2v(0) = (0 + 256) << 0 = 256
        assert_eq!(pairs[1].0, 257);
        stable(&b, &pairs);
        roundtrip(&b);
    }

    #[test]
    fn back_stable_across_pad() {
        let mut b: Blk = Blk::new();
        let mut pairs = Vec::new();
        for i in 0..20 {
            let p = b.try_push_back(i).unwrap();
            pairs.push((b.p2v(p), i));
        }
        // crossing the BUDGET=16 boundary inserts a None pad; old vaddrs must hold
        for i in 0..40 {
            let p = b.try_push_back(100 + i).unwrap();
            pairs.push((b.p2v(p), 100 + i));
            stable(&b, &pairs);
        }
        roundtrip(&b);
    }

    #[test]
    fn back_respects_max_cap() {
        let mut b: Blk = Blk::new();
        let mut ok = 0;
        for i in 0..2000 {
            match b.try_push_back(i) {
                Ok(_) => ok += 1,
                Err(_) => break,
            }
        }
        assert!(ok > 0);
        assert!(b.len() <= b.max_capacity(), "len {} > max {}", b.len(), b.max_capacity());
    }
}

// ---------------------------------------------------------------------------
// Prepend (u16, VecStore) — Append reversed
// ---------------------------------------------------------------------------
mod prepend {
    use super::*;
    type Blk = Block<'static, u64, u16, VecStore<u64, 512>, Prepend>;

    #[test]
    fn new_empty() {
        let b: Blk = Blk::new();
        assert_eq!(b.translator().shift(), 0);
        assert_eq!(b.translator().inner_offset(), 256);
    }

    #[test]
    fn front_hot_high_addrs() {
        let mut b: Blk = Blk::new();
        let mut pairs = Vec::new();
        for i in 0..10 {
            let p = b.try_push_front(i).unwrap();
            pairs.push((b.p2v(p), i));
        }
        assert_eq!(pairs[0].0, 256);
        stable(&b, &pairs);
        roundtrip(&b);
    }

    #[test]
    fn iter_is_reverse_insertion_order() {
        let mut b: Blk = Blk::new();
        for i in 0..5 {
            b.try_push_front(i).unwrap();
        }
        let vals: Vec<u64> = b.iter().copied().collect();
        assert_eq!(vals, vec![4, 3, 2, 1, 0]);
    }

}

// ---------------------------------------------------------------------------
// split / split_and_rotate — vaddr preservation via self-pointers
// ---------------------------------------------------------------------------
// each slot stores its own vaddr (value == p2v(phys)). after a block op, every
// occupied slot's value must still equal its vaddr: if the op preserves vaddrs the
// self-pointers hold; if it reindexes them, the assert fires. this is the visual
// "does the op maintain pointers" check the split path needs.
mod split {
    use super::*;
    type Blk = Block<'static, u32, u32, VecStore<u32, 16>, FixedRoot<PreOrder>>;

    ///fill to cap-full (dummy values), then a self-pointer pass: now that no more slides
    ///will run, set each occupied slot's value to its own vaddr. slides during the fill
    ///move elements (changing their vaddrs), so the self-pointer must be set last.
    fn fill_self_pointers(b: &mut Blk) {
        let root = b.insert_root(0);
        let root = b.p2v(root);
        loop {
            let Some(last) = b.last_vaddr() else { break };
            let Some(ns) = b.find_slot(b.v2p(last), true).slide else { break };
            let slot = b.slide_none(ns);
            b.insert(0, slot);
        }
        for phys in 0..b.len() {
            if b.store().slot(phys).is_some() {
                let v = b.p2v(phys);
                *b.get_mut(phys) = v;
            }
        }
    }

    ///every occupied slot: vget(its vaddr) == its vaddr.
    fn assert_self_pointers(b: &Blk) {
        for phys in 0..b.len() {
            if b.store().slot(phys).is_some() {
                let v = b.p2v(phys);
                assert_eq!(*b.vget(v), v, "phys {phys}: self-pointer broken (vaddr {v:?})");
            }
        }
    }

    #[test]
    fn split_preserves_self_pointers() {
        let mut b: Blk = Blk::new();
        fill_self_pointers(&mut b);
        assert_eq!(b.len(), b.max_capacity(), "precondition: block cap-full");
        let at = b.len() / 2;
        let right = b.split_block(at);
        assert_eq!(b.len(), at, "left half keeps [0,at)");
        assert_self_pointers(&b);
        assert_self_pointers(&right);
    }

    ///small-CAP (u32, CAP=16) block fills to shift=28 — the shift>0 regime. the doa.md
    ///spread-both-odd + de-rotated-at offset handling assumes sh=0 (the +1->high-bit
    ///wrap and `(q+at) ror R` distribution both need vaddrs spanning 0..MAX). at
    ///shift=28 vaddrs are packed in the top bits and it doesn't hold. ignored: shift>0
    ///is deferred (the full-address-space tests below cover the sh=0 regime).
    #[ignore]
    #[test]
    fn split_and_rotate_left_half_preserves_self_pointers() {
        let mut b: Blk = Blk::new();
        fill_self_pointers(&mut b);
        assert_eq!(b.len(), b.max_capacity());
        let at = b.len() / 2;
        let _right = b.split_block_and_rotate(at);
        assert_self_pointers(&b);
    }

    ///right half is spread(1) (phys i -> 2i+1). the +1 only wraps to the high bit
    ///(= MIDPOINT = `at`, placing the right in the upper vaddr half) when vaddrs
    ///span 0..MAX (full address space, shift=0). at shift>0 vaddrs are packed in
    ///the top bits and the +1 doesn't wrap usefully — so this small-CAP (shift=28)
    ///right half is genuinely unhandled here. see split_and_rotate_full_address_space
    ///for the regime where it works. ignored: needs the general de-rotate/add/
    ///re-rotate offset handling (or a full-address-space block).
    #[ignore]
    #[test]
    fn split_and_rotate_right_half_preserves_self_pointers() {
        let mut b: Blk = Blk::new();
        fill_self_pointers(&mut b);
        assert_eq!(b.len(), b.max_capacity());
        let at = b.len() / 2;
        let right = b.split_block_and_rotate(at);
        assert_self_pointers(&right);
    }

    //full address space: u16, 65536 self-pointers at shift=0 (vaddr == phys). here
    //spread(1)'s +1 wraps via ror into the high bit (= MIDPOINT = `at`), so the
    //rotation both preserves vaddrs AND places the right half in the upper half
    //(no inner=at needed). built directly (set shift=0 + grow_back + insert) to
    //skip the O(n^2) find_slot fill. this is the arena-tier block-split regime.
    type FullBlk = Block<'static, u16, u16, VecStore<u16, 65536>, FixedRoot<PreOrder>>;

    fn fill_full_self_pointers(b: &mut FullBlk) {
        b.translator_mut().set_shift(0); //fully-grown: inner=0, outer=0 => vaddr == phys
        b.store_mut().grow_back(65536);
        for p in 0..65536 {
            let v = b.p2v(p);
            b.store_mut().insert(v, p);
        }
    }

    fn assert_self_pointers_full(b: &FullBlk) {
        for phys in 0..b.len() {
            if b.store().slot(phys).is_some() {
                let v = b.p2v(phys);
                assert_eq!(*b.vget(v), v, "phys {phys}: self-pointer broken (vaddr {v:?})");
            }
        }
    }

    #[test]
    fn split_and_rotate_full_address_space() {
        let mut b: FullBlk = FullBlk::new();
        fill_full_self_pointers(&mut b);
        assert_eq!(b.len(), b.max_capacity());
        assert_eq!(b.translator().shift(), 0);
        let at = b.len() / 2;
        eprintln!("pre:   sh={} inner={} outer={} rot={}",
            b.translator().shift(), b.translator().inner_offset(),
            b.translator().outer_offset(), b.translator().rotation());
        let right = b.split_block_and_rotate(at);
        eprintln!("left:  sh={} inner={} outer={} rot={}",
            b.translator().shift(), b.translator().inner_offset(),
            b.translator().outer_offset(), b.translator().rotation());
        eprintln!("right: sh={} inner={} outer={} rot={}",
            right.translator().shift(), right.translator().inner_offset(),
            right.translator().outer_offset(), right.translator().rotation());
        //left = spread(0) on even phys (vaddr 0..at); right = spread(1) on odd phys
        //(vaddr at..MAX). the +1 wraps to the high bit so the right lands upper half.
        assert_eq!(b.translator().rotation(), 1);
        assert_eq!(right.translator().rotation(), 1);
        assert_self_pointers_full(&b);
        assert_self_pointers_full(&right);
    }

    ///the de-rotate/add-at/re-rotate goal: `at` is NOT the midpoint. a full block
    ///forces at = len/2 = MIDPOINT (both halves must fit after the ×2 spread), so
    ///this uses a half-full block (len 32768 < max 65536) and splits at 8192. the
    ///right's outer = at - MIDPOINT cancels the rotation's +MIDPOINT (the +1 wrap)
    ///and nets `at`, so the right half preserves and starts at vaddr `at`.
    #[test]
    fn split_and_rotate_non_midpoint_at() {
        let mut b: FullBlk = FullBlk::new();
        b.translator_mut().set_shift(0);
        b.store_mut().grow_back(32768);
        for p in 0..32768 {
            let v = b.p2v(p);
            b.store_mut().insert(v, p);
        }
        let at = 8192;
        eprintln!("non-mid pre:   sh={} inner={} outer={} rot={} len={} at={}",
            b.translator().shift(), b.translator().inner_offset(),
            b.translator().outer_offset(), b.translator().rotation(), b.len(), at);
        let right = b.split_block_and_rotate(at);
        eprintln!("non-mid left:  sh={} inner={} outer={} rot={} len={}",
            b.translator().shift(), b.translator().inner_offset(),
            b.translator().outer_offset(), b.translator().rotation(), b.len());
        eprintln!("non-mid right: sh={} inner={} outer={} rot={} len={}",
            right.translator().shift(), right.translator().inner_offset(),
            right.translator().outer_offset(), right.translator().rotation(), right.len());
        assert_self_pointers_full(&b);
        assert_self_pointers_full(&right);
    }

    ///repeated split_and_rotate (rot>0): the doa.md design's home turf. simulate a
    ///block that already split_and_rotate'd once at MIDPOINT — its right half has
    ///rot=1, outer=0 (left_outer = -MIDPOINT, right = that + at ror 0 = 0). splitting
    ///that full block again at MIDPOINT (rot 1->2) must still preserve: left outer =
    ///outer - (MIDPOINT ror 1) = -16384, right outer = left + (MIDPOINT ror 1) = 0.
    ///at=MIDPOINT is no-carry (q+at = MIDPOINT, single bit), so the de-rotated-at
    ///distribution holds.
    #[test]
    fn split_and_rotate_repeated_rot1() {
        let mut b: FullBlk = FullBlk::new();
        b.translator_mut().set_shift(0);
        b.translator_mut().set_rotation(1);
        b.translator_mut().set_outer_offset(0); //right half of a first at=MIDPOINT split
        b.store_mut().grow_back(65536); //full (the right half after a MIDPOINT split is full)
        for p in 0..65536 {
            let v = b.p2v(p);
            b.store_mut().insert(v, p);
        }
        let at = 32768; //MIDPOINT — no-carry, the doa.md repeated regime
        eprintln!("rot1 pre:   sh={} inner={} outer={} rot={} len={} at={}",
            b.translator().shift(), b.translator().inner_offset(),
            b.translator().outer_offset(), b.translator().rotation(), b.len(), at);
        let right = b.split_block_and_rotate(at);
        eprintln!("rot1 left:   sh={} inner={} outer={} rot={} len={}",
            b.translator().shift(), b.translator().inner_offset(),
            b.translator().outer_offset(), b.translator().rotation(), b.len());
        eprintln!("rot1 right:  sh={} inner={} outer={} rot={} len={}",
            right.translator().shift(), right.translator().inner_offset(),
            right.translator().outer_offset(), right.translator().rotation(), right.len());
        assert_eq!(b.translator().rotation(), 2);
        assert_eq!(right.translator().rotation(), 2);
        assert_self_pointers_full(&b);
        assert_self_pointers_full(&right);
    }

    //cap limited to half the address space (MIDPOINT), so min shift = 1: blocks are
    //always spreadable on both sides (a cap-full block splits at cap/2 = MIDPOINT/2,
    //each half <= cap after the x2 spread), and `at` is always a single high bit
    //(MIDPOINT/2 << sh = MIDPOINT) so q<<sh and at<<sh are disjoint — no carry, sound
    //for any R. this is the user's proposal: test split_and_rotate across generations
    //(increasing R) and various single-bit `at` values.
    type HalfBlk = Block<'static, u16, u16, VecStore<u16, 32768>, FixedRoot<PreOrder>>;
    const HALF_CAP: usize = 32768; //MIDPOINT for u16

    ///build a full (len = HALF_CAP) block at the given (rot, outer), shift=1, dense
    ///self-pointers. simulates a half-cap block that filled up at generation `rot`.
    fn make_half(rot: u32, outer: u16) -> HalfBlk {
        let mut b: HalfBlk = HalfBlk::new();
        b.translator_mut().set_shift(1);
        b.translator_mut().set_rotation(rot);
        b.translator_mut().set_outer_offset(outer);
        b.store_mut().grow_back(HALF_CAP);
        for p in 0..HALF_CAP {
            let v = b.p2v(p);
            b.store_mut().insert(v, p);
        }
        b
    }

    fn assert_sp_half(b: &HalfBlk, ctx: &str) {
        for phys in 0..b.len() {
            if b.store().slot(phys).is_some() {
                let v = b.p2v(phys);
                assert_eq!(*b.vget(v), v, "{ctx}: phys {phys} broken (vaddr {v:?})");
            }
        }
    }

    ///split_and_rotate across generations R=0,1,2,3 at at=MIDPOINT/2 (the cap-full
    ///split point). each generation takes the previous right half's (rot, outer) and
    ///re-fills to cap, then splits again. vaddrs must preserve at every generation.
    #[test]
    fn split_and_rotate_half_cap_generations() {
        let mut rot = 0u32;
        let mut outer = 0u16;
        let at = HALF_CAP / 2; //MIDPOINT/2 = 16384, single bit BW-2
        for g in 0..=3u32 {
            let mut b = make_half(rot, outer);
            assert_eq!(b.len(), b.max_capacity(), "gen {g} not cap-full");
            let right = b.split_block_and_rotate(at);
            assert_sp_half(&b, &format!("gen {g} left"));
            assert_sp_half(&right, &format!("gen {g} right"));
            //advance to the right half for the next generation
            rot = (rot + 1) % u16::BIT_WIDTH as u32;
            outer = right.translator().outer_offset();
        }
    }

    ///various single-bit `at` values at R=0 (first split — no ror-distribution, so any
    ///no-carry `at` works). each `at` is a power of two (one bit), with a non-full block
    ///(len = 2*at) so the split fits the x2 spread on both sides.
    #[test]
    fn split_and_rotate_half_cap_various_ats() {
        for &at in &[4096usize, 8192, 16384] {
            let mut b: HalfBlk = HalfBlk::new();
            b.translator_mut().set_shift(1);
            b.store_mut().grow_back(2 * at);
            for p in 0..(2 * at) {
                let v = b.p2v(p);
                b.store_mut().insert(v, p);
            }
            let right = b.split_block_and_rotate(at);
            assert_sp_half(&b, &format!("at={at} left"));
            assert_sp_half(&right, &format!("at={at} right"));
        }
    }
}
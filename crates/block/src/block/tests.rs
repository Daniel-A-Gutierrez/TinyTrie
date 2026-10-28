//!inline test battery: alignment edges, meeting-point refusals, extend/shrink
//!contracts, grow pointer-survival, staleness canaries.

use super::*;
use crate::handle::Ptr;

fn init<T: Copy>(u: &std::mem::MaybeUninit<T>) -> T {
    // SAFETY: callers write before reading
    unsafe { u.assume_init() }
}

#[test]
fn heap_alignment_edges() {
    for pre in 0..33 {
        let mut b = Block::<u16>::new(256);
        if pre > 0 {
            b.alloc::<u8>(pre).unwrap();
        }
        let mut last = b.heap_len();
        for a in [1usize, 2, 4, 8] {
            let h = match a {
                1 => b.alloc::<u8>(3).unwrap(),
                2 => b.alloc::<u16>(3).unwrap(),
                4 => b.alloc::<u32>(3).unwrap(),
                _ => b.alloc::<u64>(3).unwrap(),
            };
            let start = b.cap() - h.0.as_usize();
            assert_eq!(start % a, 0, "pre {pre} align {a}");
            assert_eq!(h.0.as_usize(), b.heap_len(), "newest H.0 == heap_len");
            assert!(h.1.as_usize() == 3 * a);
            assert!(b.heap_len() >= last + 3 * a);
            last = b.heap_len();
            //written contract: write then read back through the exact region
            for (i, x) in b.heap_mut(h).iter_mut().enumerate() {
                *x = (i % 251) as u8 + 1;
            }
            assert!(b
                .heap(h)
                .iter()
                .enumerate()
                .all(|(i, &x)| x == (i % 251) as u8 + 1));
        }
        // pad above the newest region consumed, region start T-aligned
        let h = b.alloc::<u64>(1).unwrap();
        assert_eq!((b.cap() - h.0.as_usize()) % 8, 0, "pre {pre}");
    }
}

#[test]
fn stack_alignment_edges() {
    for pre in 0..9 {
        let mut b = Block::<u16>::new(256);
        for _ in 0..pre {
            b.push::<u8>(1u8).unwrap();
        }
        let s32 = b.push::<u32>(7).unwrap();
        assert_eq!(s32.0.as_usize() % 4, 0, "pre {pre}");
        assert_eq!(*b.stack::<u32>(s32), 7);
        let s64 = b.push_buf::<u64>(2).unwrap();
        assert_eq!(s64.0.as_usize() % 8, 0, "pre {pre}");
        assert_eq!(s64.1.as_usize(), 16);
        b.stack_buf_mut::<u64>(s64)[1].write(9);
        assert_eq!(init(&b.stack_buf::<u64>(s64)[1]), 9);
        assert_eq!(b.stack_len(), s64.0.as_usize() + 16);
    }
}

#[test]
fn meeting_point_refusals() {
    let mut b = Block::<u16>::new(128);
    for i in 0..32u32 {
        assert!(b.push::<u32>(i).is_ok());
    }
    assert_eq!(b.stack_len(), 128);
    assert_eq!(b.free_bytes(), 0);
    let e = b.push::<u32>(0).unwrap_err();
    assert_eq!(e.free, 0);
    let e = b.alloc::<u8>(1).unwrap_err();
    assert_eq!(e.free, 0);
    // failed ops leave state untouched
    assert_eq!(b.stack_len(), 128);
    assert_eq!(b.heap_len(), 0);
    // shrink reopens the stack front
    b.stack_shrink(64);
    let s = b.push::<u32>(5).unwrap();
    assert_eq!(s.0.as_usize(), 64);
    // heap front eats the remaining gap exactly, both fronts then refuse
    let h = b.alloc::<u8>(48).unwrap();
    assert_eq!(b.heap_len(), 48);
    let h2 = b.alloc::<u8>(12).unwrap();
    assert_eq!(b.free_bytes(), 0);
    assert!(b.push::<u32>(0).is_err());
    assert!(b.alloc::<u8>(1).is_err());
    assert_eq!(b.heap(h2).len(), 12);
    let _ = h;
}

#[test]
fn extend_buf_contract() {
    let mut b = Block::<u16>::new(192);
    let s = b.push_buf::<u32>(2).unwrap();
    b.stack_buf_mut::<u32>(s)[0].write(1);
    b.stack_buf_mut::<u32>(s)[1].write(2);
    let s2 = b.extend_buf::<u32>(s, 2).unwrap();
    assert_eq!(s2.0, s.0);
    assert_eq!(s2.1.as_usize(), 16);
    b.stack_buf_mut::<u32>(s2)[2].write(3);
    let buf = b.stack_buf::<u32>(s2);
    assert_eq!([init(&buf[0]), init(&buf[1]), init(&buf[2])], [1, 2, 3]);
    // extend of a typed push handle (len == size) works
    let t = b.push::<u16>(8).unwrap();
    let t2 = b.extend_buf::<u16>(t, 1).unwrap();
    assert_eq!((t2.0, t2.1.as_usize()), (t.0, 4));
    // space failure leaves stack_len untouched
    let full = b.push_buf::<u32>(43).unwrap();
    let e = b.extend_buf::<u32>(full, 1).unwrap_err();
    assert_eq!(e.free, 0);
    assert_eq!(b.stack_len(), 192);
}

#[test]
#[should_panic(expected = "extend_buf")]
fn extend_buf_non_last() {
    let mut b = Block::<u16>::new(64);
    let s = b.push::<u32>(1).unwrap();
    b.push::<u8>(2).unwrap();
    let _ = b.extend_buf::<u32>(s, 1);
}

#[test]
#[should_panic(expected = "extend_buf")]
fn extend_buf_mis_sized() {
    let mut b = Block::<u16>::new(64);
    let s = b.push::<u32>(1).unwrap();
    let _ = b.extend_buf::<u64>(s, 1);
}

#[test]
fn shrink_reuse() {
    let mut b = Block::<u16>::new(128);
    let s1 = b.push::<u64>(0xDEAD).unwrap();
    b.push::<u64>(0xBEEF).unwrap();
    // both regions dead by contract
    b.stack_shrink(0);
    assert_eq!(b.stack_len(), 0);
    let a = b.push::<u64>(0xFACE).unwrap();
    let c = b.push::<u64>(0xBEEF).unwrap();
    assert_eq!(a.0.as_usize(), 0);
    assert_eq!(c.0.as_usize(), 8);
    assert_eq!(*b.stack::<u64>(a), 0xFACE);
    assert_eq!(*b.stack::<u64>(c), 0xBEEF);
    // partial shrink keeps earlier items addressable
    b.stack_shrink(8);
    assert_eq!(*b.stack::<u64>(a), 0xFACE);
    let _ = s1;
}

#[test]
#[should_panic(expected = "malformed")]
fn stale_stack_handle_rejected() {
    let mut b = Block::<u16>::new(64);
    let s = b.push::<u32>(1).unwrap();
    b.stack_shrink(s.0.as_usize());
    let _ = b.stack::<u32>(s);
}

#[test]
#[should_panic(expected = "stack_shrink")]
fn shrink_past_bounds() {
    let mut b = Block::<u16>::new(64);
    b.stack_shrink(1);
}

#[test]
#[should_panic(expected = "malformed")]
fn stack_cross_type_misalign_rejected() {
    let mut b = Block::<u16>::new(64);
    b.push::<u8>(1).unwrap();
    b.push::<u8>(2).unwrap(); // stack_len 2 — a u16 buffer lands at start 2
    let s = b.push_buf::<u16>(2).unwrap();
    let _ = b.stack::<u32>(s); // byte len fits (4) but start is not 4-aligned
}

#[test]
fn grow_preserves_handles_and_bytes() {
    let mut b = Block::<u16>::new(256);
    let mut sv = Vec::new();
    for i in 0..10u64 {
        sv.push(b.push::<u64>(i * 7 + 1).unwrap());
    }
    let h1 = b.alloc::<u8>(40).unwrap();
    let h2 = b.alloc::<u32>(8).unwrap();
    for (i, x) in b.heap_mut(h1).iter_mut().enumerate() {
        *x = (i * 3 + 1) as u8;
    }
    for (i, u) in b.heap_as_mut::<u32>(h2).iter_mut().enumerate() {
        u.write(0x1000 + i as u32);
    }
    let stack_bytes = b.stack_bytes().to_vec();
    let heap_bytes = b.heap(h1).to_vec();
    b.grow(1024).unwrap();
    assert_eq!(b.cap(), 1024);
    for (i, s) in sv.iter().enumerate() {
        assert_eq!(*b.stack::<u64>(*s), (i * 7 + 1) as u64);
    }
    assert_eq!(b.stack_bytes(), &stack_bytes[..]);
    assert_eq!(b.heap(h1), &heap_bytes[..]);
    for (i, u) in b.heap_as::<u32>(h2).iter().enumerate() {
        assert_eq!(init(u), 0x1000 + i as u32);
    }
    // sub-cap grow is a no-op; below-used is refused with current headroom
    b.grow(512).unwrap();
    assert_eq!(b.cap(), 1024);
    let used = b.stack_len() + b.heap_len();
    assert!(used <= 1024);
    let e = b.grow(140).unwrap_err();
    assert_eq!(e.free, b.free_bytes());
}

#[test]
fn pop_heap_and_reuse() {
    let mut b = Block::<u16>::new(128);
    let h1 = b.alloc::<u8>(16).unwrap();
    for (i, x) in b.heap_mut(h1).iter_mut().enumerate() {
        *x = (i as u8).wrapping_add(1);
    }
    let h2 = b.alloc::<u8>(8).unwrap();
    for x in b.heap_mut(h2) {
        *x = 7;
    }
    b.pop_heap(8);
    assert_eq!(b.heap_len(), 16);
    //the older region is intact through its exact bytes
    assert!(b
        .heap(h1)
        .iter()
        .enumerate()
        .all(|(i, &x)| x == (i as u8).wrapping_add(1)));
    // reuse lands where the popped region sat
    let h3 = b.alloc::<u8>(8).unwrap();
    assert_eq!(h3.0, h2.0);
    assert_eq!(b.heap_len(), 24);
    assert!(b
        .heap(h1)
        .iter()
        .enumerate()
        .all(|(i, &x)| x == (i as u8).wrapping_add(1)));
}

#[test]
#[should_panic(expected = "malformed or stale")]
fn stale_heap_handle_rejected() {
    let mut b = Block::<u16>::new(128);
    let h1 = b.alloc::<u8>(16).unwrap();
    let h2 = b.alloc::<u8>(8).unwrap();
    let _ = h1;
    b.pop_heap(8);
    let _ = b.heap(h2);
}

#[test]
#[should_panic(expected = "pop_heap")]
fn pop_past_empty() {
    let mut b = Block::<u16>::new(64);
    b.pop_heap(1);
}

#[test]
fn cap_rounding_and_free() {
    let b = Block::<u16>::new(100);
    assert_eq!(b.cap(), 112);
    assert_eq!(b.free_bytes(), 112);
}

#[test]
#[should_panic(expected = "cap exceeds pointer width")]
fn cap_beyond_pointer_width() {
    let _ = Block::<u8>::new(256);
}

///zero-record invariant: reclaims wipe, fresh claims read as zero — no stale
///bytes can reach a serialized record
#[test]
fn reclaims_wipe_and_claims_are_zero() {
    let mut b = Block::<u16>::new(128);
    //heap pop wipes: the reused span reads zero at the next alloc
    let h1 = b.alloc::<u8>(8).unwrap();
    for x in b.heap_mut(h1) {
        *x = 0xAB;
    }
    b.pop_heap(8);
    let h2 = b.alloc::<u8>(8).unwrap();
    assert_eq!(h2.0, h1.0);
    assert!(b.heap(h2).iter().all(|&x| x == 0));
    //stack shrink wipes: shrink past written bytes, recclaim, read zero
    let s = b.push_buf::<u8>(8).unwrap();
    let raw = b.stack_bytes_mut();
    raw[s.0.as_usize()..s.1.as_usize()].fill(0xCD);
    b.stack_shrink(s.0.as_usize());
    let s2 = b.push_buf::<u8>(8).unwrap();
    assert_eq!(s2.0, s.0);
    assert!(b.stack_bytes()[s2.0.as_usize()..s2.1.as_usize()]
        .iter()
        .all(|&x| x == 0));
    //grow: the new tail claims as zero (realloc's extension must not leak)
    let a = b.alloc::<u8>(4).unwrap();
    for x in b.heap_mut(a) {
        *x = 0xEF;
    }
    b.grow(4096).unwrap();
    let t = b.alloc::<u8>(64).unwrap();
    assert_eq!(t.1.as_usize(), 64);
    assert!(b.heap(t).iter().all(|&x| x == 0));
    //fresh block: untouched span reads zero through the heap view
    let mut c = Block::<u16>::new(64);
    let f = c.alloc::<u8>(32).unwrap();
    assert!(c.heap(f).iter().all(|&x| x == 0));
}

///A: alloc::<u32>(16) = 64B at back_off 64, C: alloc::<u8>(3) at back_off 67,
///B: alloc::<u32>(8) = 32B at back_off 100 (1 pad byte above) — heap 100
fn free_setup(b: &mut Block<u16>) -> (H<u16>, H<u16>, H<u16>) {
    let a = b.alloc::<u32>(16).unwrap();
    let c = b.alloc::<u8>(3).unwrap();
    let h = b.alloc::<u32>(8).unwrap();
    (a, h, c)
}

#[test]
fn free_records_holes_sorted() {
    let mut b = Block::<u16>::new(256);
    let (a, h1, c) = free_setup(&mut b);
    assert!(b.holes.is_empty());
    //free the newest first, then the oldest — insertion keeps the sort
    b.free(h1);
    assert_eq!(b.holes, [(100, 32)]);
    b.free(a);
    assert_eq!(b.holes, [(64, 64), (100, 32)]);
    //7c: c abuts a (span [189,192) under a's [192,256)) — coalesced
    b.free(c);
    assert_eq!(b.holes, [(67, 67), (100, 32)]);
}

///zero-record at the hole: user bytes read zero, only the 8-byte header is
///nonzero — and a sub-8 hole carries no header (it would overrun into the
///adjacent live region)
#[test]
fn free_wipes_and_stamps_header() {
    let mut b = Block::<u16>::new(256);
    let (a, _h1, _c) = free_setup(&mut b);
    for x in b.heap_mut(a) {
        *x = 0xAB;
    }
    b.free(a);
    let bytes = b.heap(a);
    assert_eq!(bytes.len(), 64);
    assert_eq!(&bytes[..8], &[0xEE, 0xF7, 0xEE, 0xF7, 64, 0, 0, 0]);
    assert!(bytes[8..].iter().all(|&x| x == 0));
    //sub-8 hole: wiped clean, no header bytes spill
    //7c: the freed 3-byte c (back_off 64) abuts the split's slack hole
    //(61, 61) — coalesced into one stamped hole
    let c = b.alloc::<u8>(3).unwrap();
    for x in b.heap_mut(c) {
        *x = 0xCD;
    }
    b.free(c);
    assert_eq!(b.holes, [(64, 64)]);
    assert_eq!(&raw(&b, 192, 8), &[0xEE, 0xF7, 0xEE, 0xF7, 64, 0, 0, 0]);
    assert!(raw(&b, 200, 56).iter().all(|&x| x == 0));
}

#[test]
fn pop_prunes_holes() {
    let mut b = Block::<u16>::new(256);
    let a = b.alloc::<u32>(8).unwrap();
    let h = b.alloc::<u32>(8).unwrap();
    b.free(h);
    b.free(a);
    //7c: a's span [224,256) abuts h's hole [192,224) — coalesced
    assert_eq!(b.holes, [(64, 64)]);
    //each pop wipes its region and prunes the hole records it rolled past
    //(the merged hole straddles the boundary — clipped to [224, 256))
    b.pop_heap(32);
    assert_eq!(b.holes, [(32, 32)]);
    b.pop_heap(32);
    assert_eq!(b.holes, []);
}

#[test]
fn rewind_restores_hole_state() {
    let mut b = Block::<u16>::new(256);
    let (a, h1, c) = free_setup(&mut b);
    b.free(a);
    let snap = b.allocator().snapshot();
    assert_eq!(snap.holes, [(64, 64)]);
    b.free(h1);
    b.free(c);
    //7c: c coalesces into a's hole
    assert_eq!(b.holes.len(), 2);
    let mut al = b.allocator();
    al.rewind(snap);
    //snapshot state restored exactly; 7d will refresh the restored headers
    assert_eq!(b.holes, [(64, 64)]);
    assert_eq!(b.heap_len(), 100);
}

#[test]
#[should_panic(expected = "double free")]
fn double_free_panics() {
    let mut b = Block::<u16>::new(64);
    let h = b.alloc::<u8>(16).unwrap();
    b.free(h);
    b.free(h);
}

//7b — alloc's hole path

fn raw(b: &Block<u16>, at: usize, n: usize) -> &[u8] {
    // SAFETY: [at, at+n) inside the data region — read-only
    unsafe { std::slice::from_raw_parts(b.data().add(at), n) }
}

#[repr(align(16))]
#[derive(Clone, Copy)]
//only its layout is consumed — alloc sizes/aligns by T
#[allow(dead_code)]
struct A16([u8; 16]);

#[test]
fn alloc_reuses_hole_exactly() {
    let mut b = Block::<u16>::new(256);
    let h1 = b.alloc::<u8>(32).unwrap();
    for x in b.heap_mut(h1) {
        *x = 0xAB;
    }
    b.free(h1);
    assert_eq!(b.holes, [(32, 32)]);
    let h2 = b.alloc::<u8>(32).unwrap();
    assert_eq!(h2, h1);
    assert_eq!(b.heap_len(), 32);
    assert!(b.holes.is_empty());
    //free's wipe minus its header, header zeroed at consume — all zero here
    assert!(b.heap(h2).iter().all(|&x| x == 0));
}

#[test]
fn alloc_aligns_within_hole() {
    let mut b = Block::<u16>::new(256);
    let a = b.alloc::<u8>(12).unwrap(); //start 244 — misaligned to 8
    for x in b.heap_mut(a) {
        *x = 0xAB;
    }
    b.free(a);
    assert_eq!(b.holes, [(12, 12)]);
    let h = b.alloc::<u64>(1).unwrap();
    assert_eq!((b.cap() - h.0.as_usize()) % 8, 0);
    assert_eq!(h.1.as_usize(), 8);
    //4-byte slack below (< 8) absorbed untracked, no split above
    assert!(b.holes.is_empty());
    assert_eq!(b.heap_len(), 12);
    //old header remnants zeroed both sides of the span
    assert!(b.heap(h).iter().all(|&x| x == 0));
    assert!(raw(&b, b.cap() - h.0.as_usize() - 4, 4).iter().all(|&x| x == 0));
}

#[test]
fn alloc_splits_slack_below_hole() {
    let mut b = Block::<u16>::new(512);
    let a = b.alloc::<u8>(28).unwrap(); //start 484, back_off 28
    for x in b.heap_mut(a) {
        *x = 0xCD;
    }
    b.free(a);
    assert_eq!(b.holes, [(28, 28)]);
    let h = b.alloc::<A16>(1).unwrap(); //16 bytes, align 16
    let start = b.cap() - h.0.as_usize();
    assert_eq!(start % 16, 0);
    //12 slack bytes below (≥ 8) → hole at the same back_off, header restamped
    assert_eq!(b.holes, [(28, 12)]);
    assert_eq!(b.heap_len(), 28);
    assert!(b.heap(h).iter().all(|&x| x == 0));
    assert_eq!(&raw(&b, 484, 8), &[0xEE, 0xF7, 0xEE, 0xF7, 12, 0, 0, 0]);
    assert!(raw(&b, 492, 4).iter().all(|&x| x == 0));
}

#[test]
fn alloc_best_fit_prefers_smallest() {
    let mut b = Block::<u16>::new(256);
    //live 8-byte spacers keep the frees non-touching (7c coalescing)
    let r1 = b.alloc::<u8>(16).unwrap(); //back_off 16, [240, 256)
    let _s1 = b.alloc::<u8>(8).unwrap(); //back_off 24
    let r2 = b.alloc::<u8>(48).unwrap(); //back_off 72, [184, 232)
    let _s2 = b.alloc::<u8>(8).unwrap(); //back_off 80
    let r3 = b.alloc::<u8>(16).unwrap(); //back_off 96, [160, 176)
    b.free(r2);
    b.free(r1);
    b.free(r3);
    assert_eq!(b.holes, [(16, 16), (72, 48), (96, 16)]);
    //two len-16 holes fit → tie goes to the lower back_off, the 48 stays put
    let h = b.alloc::<u8>(8).unwrap();
    assert_eq!((h.0.as_usize(), h.1.as_usize()), (16, 8));
    assert_eq!(b.holes, [(8, 8), (72, 48), (96, 16)]);
    assert!(b.heap(h).iter().all(|&x| x == 0));
    //8-byte split above carries its stamp
    assert_eq!(&raw(&b, 248, 8), &[0xEE, 0xF7, 0xEE, 0xF7, 8, 0, 0, 0]);
    //no hole fits → falls through below the bottom, holes untouched
    let h2 = b.alloc::<u8>(56).unwrap();
    assert_eq!((h2.0.as_usize(), h2.1.as_usize()), (152, 56));
    assert_eq!(b.heap_len(), 152);
    assert_eq!(b.holes, [(8, 8), (72, 48), (96, 16)]);
}

#[test]
fn alloc_absorbs_both_slivers() {
    let mut b = Block::<u16>::new(256);
    let a = b.alloc::<u8>(10).unwrap(); //start 246, header at [246, 254)
    for x in b.heap_mut(a) {
        *x = 0xAB;
    }
    b.free(a);
    let h = b.alloc::<u32>(1).unwrap(); //start 248, slack 2 below + 4 above
    assert_eq!((h.0.as_usize(), h.1.as_usize()), (8, 4));
    assert_eq!(b.heap_len(), 10);
    //both slacks < 8 — absorbed, the old header's full 8 bytes wiped by hand
    assert!(b.holes.is_empty());
    assert!(b.heap(h).iter().all(|&x| x == 0));
    assert!(raw(&b, 246, 10).iter().all(|&x| x == 0));
}

#[test]
fn alloc_zero_size_never_spends_a_hole() {
    let mut b = Block::<u16>::new(256);
    let a = b.alloc::<u8>(8).unwrap();
    b.free(a);
    let h = b.alloc::<u64>(0).unwrap();
    assert_eq!(h.1.as_usize(), 0);
    assert_eq!(b.holes, [(8, 8)]);
}

//7c — free's coalescing

///freed region directly above an existing hole (higher address) — successor
///merge: merged start = the hole's start, one re-stamp covers both stamps
#[test]
fn free_coalesces_hole_above() {
    let mut b = Block::<u16>::new(256);
    let r1 = b.alloc::<u8>(16).unwrap(); //[240, 256)
    let r2 = b.alloc::<u8>(16).unwrap(); //[224, 240)
    b.free(r2);
    assert_eq!(b.holes, [(32, 16)]);
    b.free(r1);
    assert_eq!(b.holes, [(32, 32)]);
    assert_eq!(&raw(&b, 224, 8), &[0xEE, 0xF7, 0xEE, 0xF7, 32, 0, 0, 0]);
    //r1's absorbed stamp erased, merge tail wiped
    assert!(raw(&b, 232, 24).iter().all(|&x| x == 0));
}

///freed region directly below an existing hole — predecessor merge: the hole's
///entry extends, its old stamp erased and the merged re-stamp lands on it
#[test]
fn free_coalesces_hole_below() {
    let mut b = Block::<u16>::new(256);
    let r1 = b.alloc::<u8>(16).unwrap(); //[240, 256)
    let r2 = b.alloc::<u8>(16).unwrap(); //[224, 240)
    b.free(r1);
    assert_eq!(b.holes, [(16, 16)]);
    b.free(r2);
    assert_eq!(b.holes, [(32, 32)]);
    assert_eq!(&raw(&b, 224, 8), &[0xEE, 0xF7, 0xEE, 0xF7, 32, 0, 0, 0]);
    assert!(raw(&b, 232, 24).iter().all(|&x| x == 0));
}

///three adjacent regions freed in spread order bridge into one hole
#[test]
fn free_bridge_coalesce() {
    let mut b = Block::<u16>::new(256);
    let r1 = b.alloc::<u8>(16).unwrap(); //[240, 256) off 16
    let r2 = b.alloc::<u8>(16).unwrap(); //[224, 240) off 32
    let r3 = b.alloc::<u8>(16).unwrap(); //[208, 224) off 48
    b.free(r1);
    b.free(r3);
    assert_eq!(b.holes, [(16, 16), (48, 16)]);
    b.free(r2);
    assert_eq!(b.holes, [(48, 48)]);
    assert_eq!(&raw(&b, 208, 8), &[0xEE, 0xF7, 0xEE, 0xF7, 48, 0, 0, 0]);
    assert!(raw(&b, 216, 40).iter().all(|&x| x == 0));
}

///a merged hole carries exactly one MAGIC record — at the merged span's start
#[test]
fn free_merged_single_stamp() {
    let mut b = Block::<u16>::new(256);
    let r1 = b.alloc::<u8>(16).unwrap(); //[240, 256)
    let r2 = b.alloc::<u8>(16).unwrap(); //[224, 240)
    b.free(r2);
    b.free(r1);
    let span = raw(&b, 224, 32);
    let magic = HOLE_MAGIC.to_le_bytes();
    let hits: Vec<usize> = span
        .windows(4)
        .enumerate()
        .filter(|(_, w)| *w == &magic)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(hits, [0]);
    assert_eq!(&span[4..8], &32u32.to_le_bytes());
}

///a sub-8 freed hole abuts a bigger hole — merges with no header remnant
///(its span never carried a stamp, so nothing to erase)
#[test]
fn free_absorbed_sub8_hole() {
    let mut b = Block::<u16>::new(256);
    let r1 = b.alloc::<u8>(4).unwrap(); //[252, 256)
    let r2 = b.alloc::<u8>(16).unwrap(); //[236, 252)
    b.free(r2);
    assert_eq!(b.holes, [(20, 16)]);
    b.free(r1);
    assert_eq!(b.holes, [(20, 20)]);
    assert_eq!(&raw(&b, 236, 8), &[0xEE, 0xF7, 0xEE, 0xF7, 20, 0, 0, 0]);
    assert!(raw(&b, 252, 4).iter().all(|&x| x == 0));
}

///the coalesced hole covers the freed region's identity — still a double free
#[test]
#[should_panic(expected = "double free")]
fn double_free_panics_after_merge() {
    let mut b = Block::<u16>::new(256);
    let r1 = b.alloc::<u8>(16).unwrap(); //[240, 256)
    let r2 = b.alloc::<u8>(16).unwrap(); //[224, 240)
    b.free(r2);
    b.free(r1);
    assert_eq!(b.holes, [(32, 32)]);
    b.free(r1);
}

//7d — rewind restore + pop-clip stamping

///consume-then-restore: a hole-path alloc's archive bytes die at rewind — the
///restored hole reads zero except its fresh header
#[test]
fn rewind_consume_then_restore() {
    let mut b = Block::<u16>::new(256);
    let h = b.alloc::<u8>(32).unwrap(); //[224, 256), back_off 32
    for x in b.heap_mut(h) {
        *x = 0xAB;
    }
    b.free(h);
    let snap = b.allocator().snapshot();
    assert_eq!(snap.holes, [(32, 32)]);
    //hole-path alloc consumes the span; the archive write covers it wholly
    {
        let mut al = b.allocator();
        let taken = al.copy_from::<u8>(&[0xCD; 32]).unwrap();
        assert_eq!(taken, h);
    }
    assert!(b.holes.is_empty());
    b.allocator().rewind(snap);
    assert_eq!(b.holes, [(32, 32)]);
    assert_eq!(b.heap_len(), 32);
    //the snapshot hole was whole-span nonzero and came back zero-except-header
    assert_eq!(
        &raw(&b, b.cap() - 32, 8),
        &[0xEE, 0xF7, 0xEE, 0xF7, 32, 0, 0, 0]
    );
    assert!(raw(&b, b.cap() - 24, 24).iter().all(|&x| x == 0));
}

///frees made after the snapshot are fully erased: restored holes exactly the
///snapshot's, dropped stamps and spans clean
#[test]
fn rewind_after_extra_free() {
    let mut b = Block::<u16>::new(256);
    let (a, h1, _c) = free_setup(&mut b);
    b.free(a);
    let snap = b.allocator().snapshot();
    assert_eq!(snap.holes, [(64, 64)]);
    b.free(h1); //(100, 32) spanning [156, 188), stamped at 156
    assert_eq!(b.holes, [(64, 64), (100, 32)]);
    b.allocator().rewind(snap);
    assert_eq!(b.holes, [(64, 64)]);
    assert_eq!(&raw(&b, 192, 8), &[0xEE, 0xF7, 0xEE, 0xF7, 64, 0, 0, 0]);
    assert!(raw(&b, 200, 56).iter().all(|&x| x == 0));
    //dropped hole's stamp erased too
    assert!(raw(&b, 156, 8).iter().all(|&x| x == 0));
}

///a merged hole straddling the pop boundary: clipped survivor re-stamped at
///the new bottom
#[test]
fn pop_straddle_clip_restamps() {
    let mut b = Block::<u16>::new(256);
    let r1 = b.alloc::<u8>(32).unwrap(); //[224, 256)
    let r2 = b.alloc::<u8>(24).unwrap(); //[200, 224)
    b.free(r1);
    b.free(r2);
    assert_eq!(b.holes, [(56, 56)]);
    assert_eq!(&raw(&b, 200, 8), &[0xEE, 0xF7, 0xEE, 0xF7, 56, 0, 0, 0]);
    b.pop_heap(32); //keep 24 — survivor [232, 256)
    assert_eq!(b.holes, [(24, 24)]);
    assert_eq!(&raw(&b, 232, 8), &[0xEE, 0xF7, 0xEE, 0xF7, 24, 0, 0, 0]);
    assert!(raw(&b, 240, 16).iter().all(|&x| x == 0));
    //the old stamp went with the wiped range
    assert!(raw(&b, 200, 8).iter().all(|&x| x == 0));
}

///a clipped survivor shorter than the 8-byte header drops its entry —
///untracked wiped gap
#[test]
fn pop_clip_sub8_drops_entry() {
    let mut b = Block::<u16>::new(256);
    let r1 = b.alloc::<u8>(32).unwrap();
    let r2 = b.alloc::<u8>(24).unwrap();
    b.free(r1);
    b.free(r2);
    b.pop_heap(49); //keep 7 — survivor [249, 256) is 7 bytes
    assert_eq!(b.heap_len(), 7);
    assert!(b.holes.is_empty());
    assert!(raw(&b, 249, 7).iter().all(|&x| x == 0));
}

//7e — hole persistence across the record boundary

///mirror of Stack::to_record at Block level: [header | stack | gap | heap]
fn record(b: &Block<u16>) -> Vec<u8> {
    let (stack, heap, cap) = (b.stack_len(), b.heap_len(), b.cap());
    let mut v = Vec::with_capacity(16 + cap);
    v.extend_from_slice(&(stack as u32).to_le_bytes());
    v.extend_from_slice(&(heap as u32).to_le_bytes());
    v.extend_from_slice(&(cap as u32).to_le_bytes());
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(b.stack_bytes());
    v.resize(16 + cap - heap, 0);
    v.extend_from_slice(raw(b, cap - heap, heap));
    v
}

///stamped ≥8 hole rebuilds from its record header; a sub-8 hole is the
///persistence boundary — runtime-only, lost on load — and the rebuilt freelist
///is live (alloc serves from it)
#[test]
fn record_rebuilds_stamped_holes() {
    let mut b = Block::<u16>::new(128);
    let h1 = b.alloc::<u8>(32).unwrap(); //[96, 128) back_off 32
    let pad = b.alloc::<u8>(4).unwrap(); //[92, 96) live — keeps the holes apart
    for x in b.heap_mut(pad) {
        *x = 0xAB;
    }
    let h2 = b.alloc::<u8>(4).unwrap(); //[88, 92) back_off 40
    let _h3 = b.alloc::<u8>(16).unwrap();
    b.free(h1);
    b.free(h2);
    assert_eq!(b.holes, [(32, 32), (40, 4)]);
    let rec = record(&b);
    let mut loaded = Block::<u16>::from_record(&rec);
    //the sub-8 hole carried no stamp — not reconstructible
    assert_eq!(loaded.holes, [(32, 32)]);
    assert_eq!(record(&loaded), rec);
    //usable: served from the rebuilt hole, erased span reads zero, slack re-holed
    let got = loaded.alloc::<u8>(24).unwrap();
    assert_eq!(got.0.as_usize(), 32);
    assert!(loaded.heap(got).iter().all(|&x| x == 0));
    assert_eq!(loaded.holes, [(8, 8)]);
}

///a hole-free record loads with empty holes — live data contains no stamps to
///misdiscover
#[test]
fn record_without_holes_loads_empty() {
    let mut b = Block::<u16>::new(128);
    let h = b.alloc::<u8>(40).unwrap();
    for (i, x) in b.heap_mut(h).iter_mut().enumerate() {
        *x = i as u8;
    }
    let rec = record(&b);
    let loaded = Block::<u16>::from_record(&rec);
    assert!(loaded.holes.is_empty());
}

///hole back_offsets are back-relative — grow's heap-tail slide leaves every
///entry and stamp untouched, and the hole stays servable at the new cap
#[test]
fn grow_keeps_hole_entries() {
    let mut b = Block::<u16>::new(128);
    let h = b.alloc::<u8>(32).unwrap();
    let _live = b.alloc::<u8>(40).unwrap();
    b.free(h);
    assert_eq!(b.holes, [(32, 32)]);
    b.grow(256).unwrap();
    assert_eq!(b.holes, [(32, 32)]);
    let reuse = b.alloc::<u8>(20).unwrap();
    assert_eq!(reuse.0.as_usize(), 32);
    assert!(b.heap(reuse).iter().all(|&x| x == 0));
    assert_eq!(b.holes, [(12, 12)]);
}
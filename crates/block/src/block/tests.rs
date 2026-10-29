//!inline test battery: alignment edges, meeting-point refusals, extend/shrink
//!contracts, grow pointer-survival, staleness canaries, and the in-band
//!freelist (lazy cache, threading, coalescing, persistence, pop-clipping,
//!rollback).

use super::*;
use crate::handle::H;

fn init<T: Copy>(u: &std::mem::MaybeUninit<T>) -> T {
    // SAFETY: callers write before reading
    unsafe { u.assume_init() }
}

///the freelist as off-sorted (off, len) pairs — materializes the cache.
fn holes(b: &mut Block) -> Vec<(u16, u16)> {
    b.holes_mut().clone()
}

///the node bytes at `at` as (len, next) LE u16s.
fn node(b: &Block, at: usize) -> (u16, u16) {
    b.node_at(at)
}

///per-arm alloc probe: start T-aligned, off == NODE + heap, span exact.
fn probe_alloc<T: Copy + ZeroPad + ReprC>(
    b: &mut Block,
    align: usize,
    want_span: usize,
) -> usize {
    let last = b.heap_len();
    let h = b.alloc::<T>(3).unwrap();
    let start = b.cap() - h.off() as usize;
    assert_eq!(start % align, 0);
    assert_eq!(h.off() as usize, NODE + b.heap_len(), "newest H.off == NODE + heap_len");
    //NODE rounding + size multiple: spans ≥ 4 and a T-size multiple
    assert_eq!(h.len() as usize, want_span);
    assert!(b.heap_len() >= last + 3 * std::mem::size_of::<T>());
    b.heap_len()
}

#[test]
fn heap_alignment_edges() {
    for pre in 0..33 {
        let mut b = Block::new(256);
        if pre > 0 {
            b.alloc::<u8>(pre).unwrap();
        }
        //3 × u8 spans 6 (NODE + size multiple), the rest are exact
        let mut last = b.heap_len();
        last = probe_alloc::<u8>(&mut b, 1, 6).max(last);
        last = probe_alloc::<u16>(&mut b, 2, 6).max(last);
        last = probe_alloc::<u32>(&mut b, 4, 12).max(last);
        last = probe_alloc::<u64>(&mut b, 8, 24).max(last);
        assert_eq!(b.heap_len(), last);
        //exact-region write/read round-trip through the u8 view
        let h = b.alloc::<u8>(48).unwrap();
        for (i, x) in b.heap_uninit_mut(h).iter_mut().enumerate() {
            x.write((i % 251) as u8 + 1);
        }
        assert!(b.heap(h).iter().enumerate().all(|(i, &x)| x == (i % 251) as u8 + 1));
        // pad above the newest region consumed, region start T-aligned
        let h = b.alloc::<u64>(1).unwrap();
        assert_eq!((b.cap() - h.off() as usize) % 8, 0, "pre {pre}");
    }
}

#[test]
fn stack_alignment_edges() {
    for pre in 0..9 {
        let mut b = Block::new(256);
        for _ in 0..pre {
            b.push::<u8>(1u8).unwrap();
        }
        let s32 = b.push::<u32>(7).unwrap();
        assert_eq!(s32.off() as usize % 4, 0, "pre {pre}");
        assert_eq!(*b.stack::<u32>(s32), 7);
        let s64 = b.push_buf::<u64>(2).unwrap();
        assert_eq!(s64.off() as usize % 8, 0, "pre {pre}");
        assert_eq!(s64.len() as usize, 16);
        b.stack_buf_mut::<u64>(s64)[1].write(9);
        assert_eq!(init(&b.stack_buf::<u64>(s64)[1]), 9);
        assert_eq!(b.stack_len(), s64.off() as usize + 16);
    }
}

///pushed alignment gaps read zero — the zero-record law at the stack front
#[test]
fn push_gaps_zero() {
    let mut b = Block::new(64);
    b.push::<u8>(1).unwrap();
    let s = b.push::<u32>(7).unwrap();
    assert_eq!(s.off() as usize, 4, "3-byte gap before the u32");
    assert_eq!(&b.stack_bytes()[1..4], &[0; 3], "gap zeroed");
    b.push::<u8>(2).unwrap();
    let s2 = b.push::<u64>(3).unwrap();
    assert_eq!(s2.off() as usize, 16, "7-byte gap before the u64");
    assert_eq!(&b.stack_bytes()[9..16], &[0; 7], "gap zeroed");
}

#[test]
fn meeting_point_refusals() {
    let mut b = Block::new(128);
    //the freelist head is not front space: 124 usable bytes
    for i in 0..31u32 {
        assert!(b.push::<u32>(i).is_ok());
    }
    assert_eq!(b.stack_len(), 124);
    assert_eq!(b.free_bytes(), 0);
    let e = b.push::<u32>(0).unwrap_err();
    assert_eq!(e.free, 0);
    let e = b.alloc::<u8>(1).unwrap_err();
    assert_eq!(e.free, 0);
    // failed ops leave state untouched
    assert_eq!(b.stack_len(), 124);
    assert_eq!(b.heap_len(), 0);
    // shrink reopens the stack front
    b.stack_shrink(64);
    let s = b.push::<u32>(5).unwrap();
    assert_eq!(s.off() as usize, 64);
    // heap front eats the remaining gap exactly, both fronts then refuse
    b.alloc::<u8>(48).unwrap();
    assert_eq!(b.heap_len(), 48);
    let h2 = b.alloc::<u8>(8).unwrap();
    assert_eq!(b.free_bytes(), 0);
    assert!(b.push::<u32>(0).is_err());
    assert!(b.alloc::<u8>(1).is_err());
    assert_eq!(b.heap(h2).len(), 8);
}

#[test]
fn extend_buf_contract() {
    let mut b = Block::new(192);
    let s = b.push_buf::<u32>(2).unwrap();
    b.stack_buf_mut::<u32>(s)[0].write(1);
    b.stack_buf_mut::<u32>(s)[1].write(2);
    let s2 = b.extend_buf::<u32>(s, 2).unwrap();
    assert_eq!(s2.off(), s.off());
    assert_eq!(s2.len() as usize, 16);
    b.stack_buf_mut::<u32>(s2)[2].write(3);
    let buf = b.stack_buf::<u32>(s2);
    assert_eq!([init(&buf[0]), init(&buf[1]), init(&buf[2])], [1, 2, 3]);
    // extend of a typed push handle (len == size) works
    let t = b.push::<u16>(8).unwrap();
    let t2 = b.extend_buf::<u16>(t, 1).unwrap();
    assert_eq!((t2.off(), t2.len() as usize), (t.off(), 4));
    // space failure leaves stack_len untouched (188 = usable bytes exactly)
    let full = b.push_buf::<u32>(42).unwrap();
    let e = b.extend_buf::<u32>(full, 1).unwrap_err();
    assert_eq!(e.free, 0);
    assert_eq!(b.stack_len(), 188, "usable = cap − NODE");
}

#[test]
#[should_panic(expected = "extend_buf")]
fn extend_buf_non_last() {
    let mut b = Block::new(64);
    let s = b.push::<u32>(1).unwrap();
    b.push::<u8>(2).unwrap();
    let _ = b.extend_buf::<u32>(s, 1);
}

//extend_buf's mis-sized contract is now unforgeable: the typed `S<T>` cannot
//name a non-last or non-multiple span, so only the non-last panic is pinnable

#[test]
fn shrink_reuse() {
    let mut b = Block::new(128);
    let s1 = b.push::<u64>(0xDEAD).unwrap();
    b.push::<u64>(0xBEEF).unwrap();
    // both regions dead by contract
    b.stack_shrink(0);
    assert_eq!(b.stack_len(), 0);
    let a = b.push::<u64>(0xFACE).unwrap();
    let c = b.push::<u64>(0xBEEF).unwrap();
    assert_eq!(a.off() as usize, 0);
    assert_eq!(c.off() as usize, 8);
    assert_eq!(*b.stack::<u64>(a), 0xFACE);
    assert_eq!(*b.stack::<u64>(c), 0xBEEF);
    // partial shrink keeps earlier items addressable
    b.stack_shrink(8);
    assert_eq!(*b.stack::<u64>(a), 0xFACE);
    let _ = s1;
}

#[test]
#[should_panic(expected = "stale")]
fn stale_stack_handle_rejected() {
    let mut b = Block::new(64);
    let s = b.push::<u32>(1).unwrap();
    b.stack_shrink(s.off() as usize);
    let _ = b.stack::<u32>(s);
}

#[test]
#[should_panic(expected = "stack_shrink")]
fn shrink_past_bounds() {
    let mut b = Block::new(64);
    b.stack_shrink(1);
}

#[test]
fn grow_preserves_handles_and_bytes() {
    let mut b = Block::new(256);
    let mut sv = Vec::new();
    for i in 0..10u64 {
        sv.push(b.push::<u64>(i * 7 + 1).unwrap());
    }
    let h1 = b.alloc::<u8>(40).unwrap();
    let h2 = b.alloc::<u32>(8).unwrap();
    for (i, x) in b.heap_uninit_mut(h1).iter_mut().enumerate() {
        x.write((i * 3 + 1) as u8);
    }
    for (i, u) in b.heap_uninit_mut(h2).iter_mut().enumerate() {
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
    for (i, u) in b.heap_uninit(h2).iter().enumerate() {
        assert_eq!(init(u), 0x1000 + i as u32);
    }
    //the freelist head slid to the new back and still reads the empty list
    assert_eq!(&b.heap_span()[b.heap_span().len() - NODE..], [0u8; 4].as_slice());
    // sub-cap grow is a no-op; below-used is refused with current headroom
    b.grow(512).unwrap();
    assert_eq!(b.cap(), 1024);
    let used = b.stack_len() + b.heap_len();
    assert!(used <= 1024 - NODE);
    let e = b.grow(140).unwrap_err();
    assert_eq!(e.free, b.free_bytes());
}

#[test]
fn pop_heap_and_reuse() {
    let mut b = Block::new(128);
    let h1 = b.alloc::<u8>(16).unwrap();
    for (i, x) in b.heap_uninit_mut(h1).iter_mut().enumerate() {
        x.write(i as u8 + 1);
    }
    let h2 = b.alloc::<u8>(8).unwrap();
    for x in b.heap_uninit_mut(h2) {
        x.write(7);
    }
    b.pop_heap(8);
    assert_eq!(b.heap_len(), 16);
    //the older region is intact through its exact bytes
    assert!(b.heap(h1).iter().enumerate().all(|(i, &x)| x == (i as u8).wrapping_add(1)));
    // reuse lands where the popped region sat
    let h3 = b.alloc::<u8>(8).unwrap();
    assert_eq!(h3.off(), h2.off());
    assert_eq!(b.heap_len(), 24);
    assert!(b.heap(h1).iter().enumerate().all(|(i, &x)| x == (i as u8).wrapping_add(1)));
}

#[test]
#[should_panic(expected = "malformed or stale")]
fn stale_heap_handle_rejected() {
    let mut b = Block::new(128);
    let h1 = b.alloc::<u8>(16).unwrap();
    let h2 = b.alloc::<u8>(8).unwrap();
    let _ = h1;
    b.pop_heap(8);
    let _ = b.heap(h2);
}

#[test]
#[should_panic(expected = "pop_heap")]
fn pop_past_empty() {
    let mut b = Block::new(64);
    b.pop_heap(1);
}

#[test]
fn cap_rounding_and_free() {
    let b = Block::new(100);
    assert_eq!(b.cap(), 112);
    assert_eq!(b.free_bytes(), 112 - NODE, "the head is not front space");
}

#[test]
#[should_panic(expected = "cap exceeds pointer width")]
fn cap_beyond_pointer_width() {
    let _ = Block::new(65536);
}

///zero-record invariant: reclaims wipe, fresh claims read as zero — no stale
///bytes can reach a serialized record
#[test]
fn reclaims_wipe_and_claims_are_zero() {
    let mut b = Block::new(128);
    //heap pop wipes: the reused span reads zero at the next alloc
    let h1 = b.alloc::<u8>(8).unwrap();
    for x in b.heap_uninit_mut(h1) {
        x.write(0xAB);
    }
    b.pop_heap(8);
    let h2 = b.alloc::<u8>(8).unwrap();
    assert_eq!(h2.off(), h1.off());
    assert!(b.heap(h2).iter().all(|&x| x == 0));
    //stack shrink wipes: shrink past written bytes, reclaim, read zero
    let s = b.push_buf::<u8>(8).unwrap();
    let raw = b.stack_bytes_mut();
    raw[s.off() as usize..s.len() as usize].fill(0xCD);
    b.stack_shrink(s.off() as usize);
    let s2 = b.push_buf::<u8>(8).unwrap();
    assert_eq!(s2.off(), s.off());
    assert!(b.stack_bytes()[s2.off() as usize..s2.len() as usize].iter().all(|&x| x == 0));
    //grow: the new tail claims as zero (realloc's extension must not leak)
    let a = b.alloc::<u8>(4).unwrap();
    for x in b.heap_uninit_mut(a) {
        x.write(0xEF);
    }
    b.grow(4096).unwrap();
    let t = b.alloc::<u8>(64).unwrap();
    assert_eq!(t.len() as usize, 64);
    assert!(b.heap(t).iter().all(|&x| x == 0));
    //fresh block: untouched span reads zero through the heap view
    let mut c = Block::new(64);
    let f = c.alloc::<u8>(32).unwrap();
    assert!(c.heap(f).iter().all(|&x| x == 0));
}

///A: alloc::<u32>(16) = 64B at off 68, C: alloc::<u8>(3) = 6B at off 74
///(NODE + size rounding), B: alloc::<u32>(8) = 32B at off 108 (2 pad bytes
///above) — heap 104
fn free_setup(b: &mut Block) -> (H<u32>, H<u32>, H<u8>) {
    let a = b.alloc::<u32>(16).unwrap();
    let c = b.alloc::<u8>(3).unwrap();
    let h = b.alloc::<u32>(8).unwrap();
    (a, h, c)
}

#[test]
fn free_threads_holes_sorted() {
    let mut b = Block::new(256);
    let (a, h1, c) = free_setup(&mut b);
    assert!(holes(&mut b).is_empty());
    //free the newest first, then the oldest — insertion keeps the sort
    b.free(h1);
    assert_eq!(holes(&mut b), [(108, 32)]);
    b.free(a);
    assert_eq!(holes(&mut b), [(68, 64), (108, 32)]);
    //c abuts a's hole from below — coalesced; the JOIN also reaches across
    //the 2-byte dead align pad between c's hole and h1's (sub-NODE gaps are
    //provably dead: live regions are ≥ NODE)
    b.free(c);
    assert_eq!(holes(&mut b), [(108, 104)]);
}

///zero-record at the hole: user bytes read zero, only the node at the hole
///start is bookkeeping — and the head names the first hole
#[test]
fn free_wipes_and_threads_node() {
    let mut b = Block::new(256);
    let (a, _h1, _c) = free_setup(&mut b);
    for x in b.heap_uninit_mut(a) {
        x.write(0xAB);
    }
    b.free(a);
    //node (len 64, next 0) LE at the hole start, rest zero
    assert_eq!(raw(&b, b.cap() - 68, 4), &[64, 0, 0, 0]);
    assert!(raw(&b, b.cap() - 68 + 4, 60).iter().all(|&x| x == 0));
    assert_eq!(node(&b, b.cap() - NODE), (0, 68), "head names the first hole");
    //the 6-byte c: freed from the hole it was served from, merges back
    let c = b.alloc::<u8>(3).unwrap();
    for x in b.heap_uninit_mut(c) {
        x.write(0xCD);
    }
    b.free(c);
    assert_eq!(holes(&mut b), [(68, 64)]);
    assert_eq!(node(&b, b.cap() - 68), (64, 0));
    assert!(raw(&b, b.cap() - 68 + 4, 60).iter().all(|&x| x == 0), "merged span wiped");
}

///data-region bytes at `at` — the byte-level view typed handles can't give.
fn raw(b: &Block, at: usize, n: usize) -> &[u8] {
    // SAFETY: [at, at+n) inside the data region — read-only
    unsafe { std::slice::from_raw_parts(b.data().add(at), n) }
}

#[test]
fn pop_prunes_holes() {
    let mut b = Block::new(256);
    let a = b.alloc::<u32>(8).unwrap();
    let h = b.alloc::<u32>(8).unwrap();
    b.free(h);
    b.free(a);
    //a's span abuts h's hole — coalesced
    assert_eq!(holes(&mut b), [(68, 64)]);
    //each pop wipes its region and prunes the holes it rolled past
    //(the merged hole straddles the boundary — clipped to [220, 252))
    b.pop_heap(32);
    assert_eq!(holes(&mut b), [(36, 32)]);
    b.pop_heap(32);
    assert_eq!(holes(&mut b), []);
}

#[test]
#[should_panic(expected = "double free")]
fn double_free_panics() {
    let mut b = Block::new(64);
    let h = b.alloc::<u8>(16).unwrap();
    b.free(h);
    b.free(h);
}

//alloc's hole path

#[test]
fn alloc_reuses_hole_exactly() {
    let mut b = Block::new(256);
    let h1 = b.alloc::<u8>(32).unwrap();
    for x in b.heap_uninit_mut(h1) {
        x.write(0xAB);
    }
    b.free(h1);
    assert_eq!(holes(&mut b), [(36, 32)]);
    let h2 = b.alloc::<u8>(32).unwrap();
    assert_eq!((h2.off(), h2.len()), (36, 32));
    assert_eq!(b.heap_len(), 32);
    assert!(holes(&mut b).is_empty());
    //free's wipe minus its node, node zeroed at consume — all zero here
    assert!(b.heap(h2).iter().all(|&x| x == 0));
}

#[test]
fn alloc_aligns_within_hole() {
    let mut b = Block::new(256);
    let a = b.alloc::<u8>(12).unwrap();
    for x in b.heap_uninit_mut(a) {
        x.write(0xAB);
    }
    b.free(a);
    assert_eq!(holes(&mut b), [(16, 12)]);
    let h = b.alloc::<u64>(1).unwrap();
    assert_eq!((b.cap() - h.off() as usize) % 8, 0);
    assert_eq!(h.len() as usize, 8);
    //4-byte slack below (== NODE) re-holed, no split above
    assert_eq!(holes(&mut b), [(8, 4)]);
    assert_eq!(b.heap_len(), 12);
    //old node remnants zeroed both sides of the span
    assert!(b.heap(h).iter().all(|&x| x == 0));
}

#[test]
fn alloc_splits_slack_below_hole() {
    let mut b = Block::new(512);
    let a = b.alloc::<u8>(28).unwrap();
    for x in b.heap_uninit_mut(a) {
        x.write(0xCD);
    }
    b.free(a);
    assert_eq!(holes(&mut b), [(32, 28)]);
    let h = b.alloc::<A16>(1).unwrap(); //16 bytes, align 16
    let start = b.cap() - h.off() as usize;
    assert_eq!(start % 16, 0);
    //12 slack bytes below (≥ NODE) → hole at the same off as the taken start
    assert_eq!(holes(&mut b), [(16, 12)]);
    assert_eq!(b.heap_len(), 28);
    assert!(b.heap(h).iter().all(|&x| x == A16([0; 16])));
    //the re-holed slack carries a fresh node
    assert_eq!(node(&b, b.cap() - 16), (12, 0));
    assert_eq!(b.head_next(), 16);
}

#[test]
fn alloc_best_fit_prefers_smallest() {
    let mut b = Block::new(256);
    //live 8-byte spacers keep the frees non-touching (coalescing)
    let r1 = b.alloc::<u8>(16).unwrap(); //off 20, [236, 252)
    let _s1 = b.alloc::<u8>(8).unwrap(); //off 28
    let r2 = b.alloc::<u8>(48).unwrap(); //off 76, [180, 228)
    let _s2 = b.alloc::<u8>(8).unwrap(); //off 84
    let r3 = b.alloc::<u8>(16).unwrap(); //off 100, [156, 172)
    b.free(r2);
    b.free(r1);
    b.free(r3);
    assert_eq!(holes(&mut b), [(20, 16), (76, 48), (100, 16)]);
    //two len-16 holes fit → tie goes to the lowest off, the 48 stays put
    let h = b.alloc::<u8>(8).unwrap();
    assert_eq!((h.off() as usize, h.len() as usize), (20, 8));
    assert_eq!(holes(&mut b), [(12, 8), (76, 48), (100, 16)]);
    assert!(b.heap(h).iter().all(|&x| x == 0));
    //8-byte split above carries its node
    assert_eq!(node(&b, b.cap() - 12), (8, 76));
    //no hole fits → falls through below the bottom, holes untouched
    let h2 = b.alloc::<u8>(56).unwrap();
    assert_eq!((h2.off() as usize, h2.len() as usize), (156, 56));
    assert_eq!(b.heap_len(), 152);
    assert_eq!(holes(&mut b), [(12, 8), (76, 48), (100, 16)]);
}

#[test]
fn alloc_absorbs_both_slivers() {
    let mut b = Block::new(256);
    let a = b.alloc::<u8>(10).unwrap();
    for x in b.heap_uninit_mut(a) {
        x.write(0xAB);
    }
    b.free(a);
    let h = b.alloc::<u32>(1).unwrap();
    assert_eq!((h.off() as usize, h.len() as usize), (12, 4));
    assert_eq!(b.heap_len(), 10);
    //the 2-byte align slack is < NODE — absorbed as a dead gap, ZEROED (the
    //taken hole's node sat at the hole start); the 4-byte slack below re-holed
    assert_eq!(holes(&mut b), [(8, 4)]);
    assert!(b.heap(h).iter().all(|&x| x == 0));
    assert!(raw(&b, b.cap() - 14, 2).iter().all(|&x| x == 0), "absorbed gap zero");
}

#[test]
fn alloc_zero_size_never_spends_a_hole() {
    let mut b = Block::new(256);
    let a = b.alloc::<u8>(8).unwrap();
    b.free(a);
    let h = b.alloc::<u64>(0).unwrap();
    assert_eq!(h.len() as usize, 0);
    assert_eq!(holes(&mut b), [(12, 8)]);
}

//free's coalescing

///freed region directly above an existing hole (higher address) — successor
///merge: one node at the merged start
#[test]
fn free_coalesces_hole_above() {
    let mut b = Block::new(256);
    let r1 = b.alloc::<u8>(16).unwrap(); //[236, 252)
    let r2 = b.alloc::<u8>(16).unwrap(); //[220, 236)
    b.free(r2);
    assert_eq!(holes(&mut b), [(36, 16)]);
    b.free(r1);
    assert_eq!(holes(&mut b), [(36, 32)]);
    assert_eq!(node(&b, b.cap() - 36), (32, 0));
    //r2's absorbed node erased, merge tail wiped
    assert!(b.heap(r2)[4..].iter().all(|&x| x == 0));
}

///freed region directly below an existing hole — predecessor merge: the node
///moves to the merged start, the old one erased
#[test]
fn free_coalesces_hole_below() {
    let mut b = Block::new(256);
    let r1 = b.alloc::<u8>(16).unwrap(); //[236, 252)
    let r2 = b.alloc::<u8>(16).unwrap(); //[220, 236)
    b.free(r1);
    assert_eq!(holes(&mut b), [(20, 16)]);
    b.free(r2);
    assert_eq!(holes(&mut b), [(36, 32)]);
    assert_eq!(node(&b, b.cap() - 36), (32, 0));
    assert!(b.heap(r2)[4..].iter().all(|&x| x == 0));
}

///three adjacent regions freed in spread order bridge into one hole
#[test]
fn free_bridge_coalesce() {
    let mut b = Block::new(256);
    let r1 = b.alloc::<u8>(16).unwrap(); //off 20, [236, 252)
    let r2 = b.alloc::<u8>(16).unwrap(); //off 36, [220, 236)
    let r3 = b.alloc::<u8>(16).unwrap(); //off 52, [204, 220)
    b.free(r1);
    b.free(r3);
    assert_eq!(holes(&mut b), [(20, 16), (52, 16)]);
    b.free(r2);
    assert_eq!(holes(&mut b), [(52, 48)]);
    assert_eq!(node(&b, b.cap() - 52), (48, 0));
    assert!(b.heap(r2)[4..].iter().all(|&x| x == 0));
}

///a merged hole carries exactly one node — at the merged span's start
#[test]
fn free_merged_single_node() {
    let mut b = Block::new(256);
    let r1 = b.alloc::<u8>(16).unwrap(); //[236, 252)
    let r2 = b.alloc::<u8>(16).unwrap(); //[220, 236)
    b.free(r2);
    b.free(r1);
    //node at the merged start, zero everywhere else in the span
    assert_eq!(node(&b, b.cap() - 36), (32, 0));
    assert!(b.heap(r2)[4..].iter().all(|&x| x == 0));
}

///the coalesced hole covers the freed region's identity — still a double free
#[test]
#[should_panic(expected = "double free")]
fn double_free_panics_after_merge() {
    let mut b = Block::new(256);
    let r1 = b.alloc::<u8>(16).unwrap(); //[236, 252)
    let r2 = b.alloc::<u8>(16).unwrap(); //[220, 236)
    b.free(r2);
    b.free(r1);
    assert_eq!(holes(&mut b), [(36, 32)]);
    b.free(r1);
}

//rollback (the unwind's second half) + pop-clip threading

///consume-then-release: the unwind's release half puts a taken hole back —
///fresh node, span zero (release_slice → free → node rewrite)
#[test]
fn rollback_kills_fresh_allocs() {
    let mut b = Block::new(256);
    let h = b.alloc::<u8>(32).unwrap();
    for x in b.heap_uninit_mut(h) {
        x.write(0xAB);
    }
    b.free(h);
    let heap0 = b.heap_len();
    //hole-path alloc consumes the span; the archive write covers it wholly
    let taken = {
        let mut al = b.allocator();
        al.alloc_slice::<u8>(&[0xCD; 32]).unwrap()
    };
    assert_eq!((taken.off(), taken.len()), (36, 32));
    assert!(b.holes_mut().is_empty(), "hole consumed");
    //the unwind protocol: release (children first), then the roll — here the
    //front never moved, so the roll is a no-op and the release alone restores
    {
        let mut al = b.allocator();
        al.free(taken);
        al.rollback(heap0);
    }
    assert_eq!(b.heap_len(), 32);
    assert_eq!(holes(&mut b), [(36, 32)]);
    assert_eq!(node(&b, b.cap() - 36), (32, 0));
    assert!(b.heap(h)[4..].iter().all(|&x| x == 0));
}

///rollback kills ONLY what the failed archive extended: a fresh alloc below
///the entry front dies wholesale with the roll (its freed hole included);
///pre-existing holes above ride through untouched — archive impls never
///free, so nothing else can sit below the entry front
#[test]
fn rollback_after_extra_free() {
    let mut b = Block::new(256);
    let (a, _h1, _c) = free_setup(&mut b);
    b.free(a);
    let heap0 = b.heap_len();
    assert_eq!(holes(&mut b), [(68, 64)]);
    //the failed archive's shape: fresh alloc below the entry front (sized
    //past every hole so best-fit can't serve it), freed
    let fresh = b.alloc::<u8>(100).unwrap();
    b.free(fresh);
    assert_eq!(holes(&mut b), [(68, 64), (208, 100)]);
    b.allocator().rollback(heap0);
    assert_eq!(holes(&mut b), [(68, 64)]);
    assert_eq!(node(&b, b.cap() - 68), (64, 0));
    assert!(b.heap(a)[4..].iter().all(|&x| x == 0));
    //the rolled span wiped
    assert!(raw(&b, b.cap() - 208, 100).iter().all(|&x| x == 0));
}

///the unwind's headline: a freshly-freed region ABUTTING a pre-existing
///hole at the entry bottom coalesces, and rollback's straddle clip then
///restores the pre-existing hole exactly
#[test]
fn rollback_joins_then_clips() {
    let mut b = Block::new(256);
    let pre = b.alloc::<u8>(16).unwrap(); //off 20, [236, 252)
    b.free(pre);
    let heap0 = b.heap_len();
    //the failed archive's shape: one fresh alloc directly below the entry
    //bottom (sized past the hole so best-fit serves fresh), freed — the
    //unwind's release half
    let fresh = b.alloc::<u8>(32).unwrap(); //off 52, [204, 236)
    assert_eq!(fresh.off(), 52);
    b.free(fresh);
    assert_eq!(holes(&mut b), [(52, 48)], "fresh hole joined pre's");
    b.allocator().rollback(heap0); //pop 32 — floor 20
    assert_eq!(holes(&mut b), [(20, 16)], "clipped back to pre's hole");
    assert_eq!(node(&b, b.cap() - 20), (16, 0));
}

///a merged hole straddling the pop boundary: clipped survivor re-threaded at
///the new bottom
#[test]
fn pop_straddle_clip_rethreads() {
    let mut b = Block::new(256);
    let r1 = b.alloc::<u8>(32).unwrap(); //off 36, [220, 252)
    let r2 = b.alloc::<u8>(24).unwrap(); //off 60, [196, 220)
    b.free(r1);
    b.free(r2);
    assert_eq!(holes(&mut b), [(60, 56)]);
    assert_eq!(node(&b, b.cap() - 60), (56, 0));
    b.pop_heap(32); //keep 24 — survivor [228, 252); r1 straddles and goes stale
    assert_eq!(holes(&mut b), [(28, 24)]);
    assert_eq!(node(&b, b.cap() - 28), (24, 0));
    assert!(raw(&b, b.cap() - 28 + 4, 20).iter().all(|&x| x == 0), "hole wiped past its node");
    //the old node went with the wiped range
    assert!(raw(&b, b.cap() - 60, 32).iter().all(|&x| x == 0));
}

///a clipped survivor shorter than a node drops its entry — untracked dead gap
#[test]
fn pop_clip_sub_node_drops_entry() {
    let mut b = Block::new(256);
    let r1 = b.alloc::<u8>(32).unwrap();
    let r2 = b.alloc::<u8>(24).unwrap();
    b.free(r1);
    b.free(r2);
    b.pop_heap(53); //keep 3 — survivor [252, 256) is 3 bytes
    assert_eq!(b.heap_len(), 3);
    assert!(holes(&mut b).is_empty());
}

//hole persistence across the record boundary

///mirror of Stack::to_record at Block level: [header | stack | gap | heap
///incl. head]
fn record(b: &Block) -> Vec<u8> {
    let (stack, heap, cap) = (b.stack_len(), b.heap_len(), b.cap());
    let mut v = Vec::with_capacity(16 + cap);
    v.extend_from_slice(&(stack as u32).to_le_bytes());
    v.extend_from_slice(&(heap as u32).to_le_bytes());
    v.extend_from_slice(&(cap as u32).to_le_bytes());
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(b.stack_bytes());
    v.resize(16 + cap - NODE - heap, 0);
    v.extend_from_slice(b.heap_span());
    v
}

///every hole (≥ NODE by construction) threads through the record and
///rebuilds on load — and the rebuilt freelist is live (alloc serves from it)
#[test]
fn record_rebuilds_holes() {
    let mut b = Block::new(128);
    let h1 = b.alloc::<u8>(32).unwrap(); //[92, 124), off 36
    let pad = b.alloc::<u8>(4).unwrap(); //[88, 92) live — keeps the holes apart
    for x in b.heap_uninit_mut(pad) {
        x.write(0xAB);
    }
    let h2 = b.alloc::<u8>(4).unwrap(); //[84, 88), off 44
    let _h3 = b.alloc::<u8>(16).unwrap();
    b.free(h1);
    b.free(h2);
    assert_eq!(holes(&mut b), [(36, 32), (44, 4)]);
    let rec = record(&b);
    let mut loaded = Block::from_record(&rec);
    assert_eq!(holes(&mut loaded), [(36, 32), (44, 4)], "the list IS the record");
    assert_eq!(record(&loaded), rec);
    //usable: served from the rebuilt hole, erased span reads zero, slack re-holed
    let got = loaded.alloc::<u8>(24).unwrap();
    assert_eq!(got.off() as usize, 36);
    assert!(loaded.heap(got).iter().all(|&x| x == 0));
    assert_eq!(holes(&mut loaded), [(12, 8), (44, 4)]);
}

///a corrupt freelist link truncates instead of reading wild — the holes up
///the bad link survive, the rest die, and the block still serves
#[test]
fn corrupt_list_truncates_on_load() {
    let mut b = Block::new(128);
    let h1 = b.alloc::<u8>(16).unwrap();
    let _live = b.alloc::<u8>(16).unwrap();
    let h2 = b.alloc::<u8>(16).unwrap();
    b.free(h1);
    b.free(h2);
    assert_eq!(holes(&mut b), [(20, 16), (52, 16)]);
    let mut rec = record(&b);
    //h1's node `next` (which names h2's hole) now points at a wild offset
    let node_at = 16 + 128 - h1.off() as usize;
    rec[node_at + 2..node_at + 4].copy_from_slice(&0xFFFFu16.to_le_bytes());
    let mut loaded = Block::from_record(&rec);
    assert_eq!(holes(&mut loaded), [(20, 16)], "hole past the bad link dropped");
    assert!(loaded.alloc::<u8>(8).is_ok(), "block still serves");
}

///a hole-free record loads with the empty list — live data reads as the
///zeroed head node
#[test]
fn record_without_holes_loads_empty() {
    let mut b = Block::new(128);
    let h = b.alloc::<u8>(40).unwrap();
    for (i, x) in b.heap_uninit_mut(h).iter_mut().enumerate() {
        x.write(i as u8);
    }
    let rec = record(&b);
    let mut loaded = Block::from_record(&rec);
    assert!(holes(&mut loaded).is_empty());
}

///hole links are back-relative — grow's heap-tail slide (head included)
///leaves every entry untouched, and the hole stays servable at the new cap
#[test]
fn grow_keeps_hole_entries() {
    let mut b = Block::new(128);
    let h = b.alloc::<u8>(32).unwrap();
    let _live = b.alloc::<u8>(40).unwrap();
    b.free(h);
    assert_eq!(holes(&mut b), [(36, 32)]);
    b.grow(256).unwrap();
    assert_eq!(holes(&mut b), [(36, 32)]);
    assert_eq!(b.head_next(), 36);
    let reuse = b.alloc::<u8>(20).unwrap();
    assert_eq!(reuse.off() as usize, 36);
    assert!(b.heap(reuse).iter().all(|&x| x == 0));
    assert_eq!(holes(&mut b), [(16, 12)]);
}

#[repr(align(16))]
#[derive(Clone, Copy, PartialEq)]
//only its layout is consumed — alloc sizes/aligns by T
#[allow(dead_code)]
struct A16([u8; 16]);

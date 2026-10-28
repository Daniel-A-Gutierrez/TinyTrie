//!archival + comparison over the block seam: `Archive` shoves K into the
//!block's heap through `Allocator` (the write seam) and returns a fixed-size
//!POD `Handle`; `RelOrd` compares a stored handle against a native query
//!FIELD-WISE through `Heap` (the read seam), short-circuiting at the first
//!unequal field — no view construction on the compare path. no `Ord` bound
//!lives on either trait; primitive impls carry the order, composites zip
//!fields through per-field RelOrd.
//!invariants:
//!- a FAILED archive leaves the heap unchanged: multi-alloc impls snapshot at
//!  entry and `rewind` on any Err (Allocator::snapshot/rewind)
//!- handles are plain POD with EXPLICIT padding only: every byte defined, pad
//!  bytes zero-written — pushed records never carry undefined bytes
//!- `A::HEAP == false` ⇒ the handle IS the value; Vec<A> lays the element
//!  handles into ONE region when its element doesn't heap, else archives
//!  each element individually below a handles region
//!- `Archive::release` releases every region a handle transitively names,
//!  children first; exclusive ownership, release-once
//!- strings ≤ 8 bytes ride the prefix entirely (h.0 == 0 = inline); heap
//!  regions satisfy the block's h1 ≤ h0 form, so the inline form (h.1 =
//!  length) panics if it ever reaches `Heap::bytes`

use std::cmp::Ordering;
use std::mem::MaybeUninit;

use crate::block::{Allocator, Block, CapErr};
use crate::handle::{H, Ptr};

///heap READ seam: handle→bytes lookups only — a find path needs only &Block
///to build one; release goes through Allocator (the write seam).
pub struct Heap<'a, P> {
    block: &'a Block<P>,
}

impl<'a, P: Ptr> Heap<'a, P> {
    pub fn new(block: &'a Block<P>) -> Self {
        Heap { block }
    }

    ///region bytes; panics on malformed or stale handles.
    #[inline]
    pub fn bytes(&self, h: H<P>) -> &[u8] {
        self.block.heap(h)
    }

    ///region as T slots — uninitialized view; only archive-written slots may
    ///be assumed init.
    #[inline]
    pub fn region<T>(&self, h: H<P>) -> &[MaybeUninit<T>] {
        self.block.heap_as::<T>(h)
    }

    ///slot `i` of a T region as initialized — caller guarantees archive wrote
    ///a valid handle there (handles are plain POD).
    #[inline]
    pub fn handle<T: Copy>(&self, h: H<P>, i: usize) -> &T {
        // SAFETY: caller contract — archive wrote a valid T at slot i
        unsafe { self.region::<T>(h)[i].assume_init_ref() }
    }
}

///storage only: shove K into the block, get a handle. no Ord bound, no cmp.
pub trait Archive<P: Ptr>: Sized {
    ///fixed-size slot representation; plain POD with EXPLICIT padding only —
    ///every byte defined, padded bytes zero-written (records read zero).
    ///`Eq` structural.
    type Handle: Copy + Eq;
    ///false = the handle IS the value; the heap is never touched.
    const HEAP: bool;

    ///appends k's heap regions via `al`, returns the handle; on Err the heap
    ///is unchanged.
    fn archive(k: &Self, al: &mut Allocator<'_, P>) -> Result<Self::Handle, CapErr>;

    ///releases every heap region the handle transitively names — children
    ///before the region naming them; no-op when HEAP is false. the handle
    ///owns its transitive regions EXCLUSIVELY (no sharing): release-once,
    ///releasing twice is a caller bug.
    fn release(h: &Self::Handle, al: &mut Allocator<'_, P>);
}

///field-wise comparison; every branch short-circuits at the first unequal
///field and touches only the fields it reads.
pub trait RelOrd<P: Ptr>: Archive<P> {
    ///stored handle vs native k — the tier find order.
    fn rel_cmp(h: &Self::Handle, k: &Self, heap: &Heap<P>) -> Ordering;
    ///handle vs handle over the shared heap.
    fn rel_cmp_h(a: &Self::Handle, b: &Self::Handle, heap: &Heap<P>) -> Ordering;
}

///first ≤8 bytes zero-padded, BE-loaded — the inline compare form (integer
///compare == byte-order compare).
#[inline]
fn prefix64(bytes: &[u8]) -> u64 {
    let mut buf = [0u8; 8];
    let n = bytes.len().min(8);
    buf[..n].copy_from_slice(&bytes[..n]);
    u64::from_be_bytes(buf)
}

///string handle: BE prefix + heap range. h.0 == 0 marks INLINE (≤8 bytes —
///the whole string rides pfx, h.1 = length); the block's h1 ≤ h0 region form
///makes an inline handle panic if it reaches Heap::bytes. heap handles carry
///the full bytes; h.1 = full length both ways (cmp needs it in-line).
///`_pad` = explicit trailing pad (P::TailPad2) — zero in every record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct StrHandle<P: Ptr> {
    pub pfx: u64,
    pub h: H<P>,
    _pad: P::TailPad2,
}

impl<P: Ptr> Archive<P> for String {
    type Handle = StrHandle<P>;
    const HEAP: bool = true;

    fn archive(k: &Self, al: &mut Allocator<'_, P>) -> Result<Self::Handle, CapErr> {
        let bytes = k.as_bytes();
        let len = bytes.len();
        let pfx = prefix64(bytes);
        //short-string cutoff: the whole key rides pfx, no heap bytes
        if len <= 8 {
            return Ok(StrHandle {
                pfx,
                h: H(P::from_usize(0), P::from_usize(len)),
                _pad: Default::default(),
            });
        }
        let h = al.copy_from::<u8>(bytes)?;
        Ok(StrHandle { pfx, h, _pad: Default::default() })
    }

    fn release(h: &Self::Handle, al: &mut Allocator<'_, P>) {
        //inline handles name no region (h.0 == 0 marker)
        if h.h.0 != P::from_usize(0) {
            al.free(h.h);
        }
    }
}

impl<P: Ptr> RelOrd<P> for String {
    fn rel_cmp(h: &Self::Handle, k: &Self, heap: &Heap<P>) -> Ordering {
        let kb = k.as_bytes();
        match h.pfx.cmp(&prefix64(kb)) {
            //pfx-equal: inline h is a proper prefix of (or equal-pad-vs) the
            //query — length decides
            Ordering::Equal if h.h.0 == P::from_usize(0) => h.h.1.as_usize().cmp(&kb.len()),
            Ordering::Equal => heap.bytes(h.h).cmp(kb),
            ord => ord,
        }
    }

    fn rel_cmp_h(a: &Self::Handle, b: &Self::Handle, heap: &Heap<P>) -> Ordering {
        match a.pfx.cmp(&b.pfx) {
            Ordering::Equal => match (a.h.0 == P::from_usize(0), b.h.0 == P::from_usize(0)) {
                //prefix-equal inline handles order by length
                (true, true) => a.h.1.cmp(&b.h.1),
                //pfx-equal inline is a prefix of the heap string — always <
                (true, false) => Ordering::Less,
                (false, true) => Ordering::Greater,
                (false, false) => heap.bytes(a.h).cmp(heap.bytes(b.h)),
            },
            ord => ord,
        }
    }
}

impl<P: Ptr> Archive<P> for char {
    type Handle = char;
    const HEAP: bool = false;

    fn archive(k: &Self, _al: &mut Allocator<'_, P>) -> Result<Self::Handle, CapErr> {
        Ok(*k)
    }

    fn release(_h: &Self::Handle, _al: &mut Allocator<'_, P>) {}
}

impl<P: Ptr> RelOrd<P> for char {
    fn rel_cmp(h: &Self::Handle, k: &Self, _heap: &Heap<P>) -> Ordering {
        h.cmp(k)
    }

    fn rel_cmp_h(a: &Self::Handle, b: &Self::Handle, _heap: &Heap<P>) -> Ordering {
        a.cmp(b)
    }
}

///vec handle: `h` names the handles region (the flat byte region when the
///element doesn't heap), `len` the element count. `_pad` (P::TailPad3) keeps
///the handle a multiple of 8 at every width — 8/8/16/24 — so derived
///composites can align boundaries without width-dependent gaps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct VecH<P: Ptr> {
    pub h: H<P>,
    pub len: P,
    _pad: P::TailPad3,
}

impl<P: Ptr, A: Archive<P>> Archive<P> for Vec<A> {
    type Handle = VecH<P>;
    const HEAP: bool = true;

    fn archive(k: &Self, al: &mut Allocator<'_, P>) -> Result<Self::Handle, CapErr> {
        let len = k.len();
        if len == 0 {
            return Ok(VecH { h: H(P::from_usize(0), P::from_usize(0)), len: P::from_usize(0), _pad: Default::default() });
        }
        if !A::HEAP {
            //flat: ONE region holds the element handles directly (inline
            //types: handle = value, so the loop memcpy's)
            let snap = al.snapshot();
            let rh = match al.alloc::<A::Handle>(len) {
                Ok(h) => h,
                Err(e) => {
                    al.rewind(snap);
                    return Err(e);
                }
            };
            for (i, e) in k.iter().enumerate() {
                let eh = match A::archive(e, al) {
                    Ok(h) => h,
                    Err(e) => {
                        al.rewind(snap);
                        return Err(e);
                    }
                };
                al.write_at::<A::Handle>(rh, i, eh);
            }
            return Ok(VecH { h: rh, len: P::from_usize(len), _pad: Default::default() });
        }
        let snap = al.snapshot();
        let rh = match al.alloc::<A::Handle>(len) {
            Ok(h) => h,
            Err(e) => {
                al.rewind(snap);
                return Err(e);
            }
        };
        //element regions grow below the handles region; back-relative H
        //survives — write each handle as it lands
        for (i, e) in k.iter().enumerate() {
            let eh = match A::archive(e, al) {
                Ok(h) => h,
                Err(e) => {
                    al.rewind(snap);
                    return Err(e);
                }
            };
            al.write_at::<A::Handle>(rh, i, eh);
        }
        Ok(VecH { h: rh, len: P::from_usize(len), _pad: Default::default() })
    }

    fn release(h: &Self::Handle, al: &mut Allocator<'_, P>) {
        //children first, then the region naming them; the copy un-borrows the
        //child handle before the recursive release
        for i in 0..h.len.as_usize() {
            let eh = *al.handle::<A::Handle>(h.h, i);
            A::release(&eh, al);
        }
        if h.len.as_usize() > 0 {
            al.free(h.h);
        }
    }
}

impl<P: Ptr, A: RelOrd<P>> RelOrd<P> for Vec<A> {
    fn rel_cmp(h: &Self::Handle, k: &Self, heap: &Heap<P>) -> Ordering {
        let n = h.len.as_usize().min(k.len());
        for i in 0..n {
            let eh = heap.handle::<A::Handle>(h.h, i);
            let ord = A::rel_cmp(eh, &k[i], heap);
            if ord != Ordering::Equal {
                return ord;
            }
        }
        h.len.as_usize().cmp(&k.len())
    }

    fn rel_cmp_h(a: &Self::Handle, b: &Self::Handle, heap: &Heap<P>) -> Ordering {
        let n = a.len.as_usize().min(b.len.as_usize());
        for i in 0..n {
            let ord = A::rel_cmp_h(
                heap.handle::<A::Handle>(a.h, i),
                heap.handle::<A::Handle>(b.h, i),
                heap,
            );
            if ord != Ordering::Equal {
                return ord;
            }
        }
        a.len.cmp(&b.len)
    }
}

///inline primitives: handle = value, cmp = Ord, heap never touched.
macro_rules! impl_inline {
    ($($t:ty),* $(,)?) => {$(
        impl<P: Ptr> Archive<P> for $t {
            type Handle = $t;
            const HEAP: bool = false;
            #[inline]
            fn archive(k: &Self, _al: &mut Allocator<'_, P>) -> Result<Self::Handle, CapErr> {
                Ok(*k)
            }
            #[inline]
            fn release(_h: &Self::Handle, _al: &mut Allocator<'_, P>) {}
        }
        impl<P: Ptr> RelOrd<P> for $t {
            #[inline]
            fn rel_cmp(h: &Self::Handle, k: &Self, _heap: &Heap<P>) -> Ordering { h.cmp(k) }
            #[inline]
            fn rel_cmp_h(a: &Self::Handle, b: &Self::Handle, _heap: &Heap<P>) -> Ordering {
                a.cmp(b)
            }
        }
    )*};
}

impl_inline!(u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, i128, isize);

#[cfg(test)]
mod tests {
    use super::*;

    ///archive every k into one block; both cmp forms match Ord over all pairs
    fn assert_cmp<K: RelOrd<u32> + Ord>(ks: &[K]) {
        let mut b = Block::<u32>::new(1 << 16);
        let hs = {
            let mut al = b.allocator();
            ks.iter().map(|k| K::archive(k, &mut al).unwrap()).collect::<Vec<_>>()
        };
        let heap = Heap::new(&b);
        for (i, a) in ks.iter().enumerate() {
            for (j, c) in ks.iter().enumerate() {
                assert_eq!(K::rel_cmp(&hs[i], c, &heap), a.cmp(c), "pair ({i},{j})");
                assert_eq!(K::rel_cmp_h(&hs[i], &hs[j], &heap), a.cmp(c));
            }
        }
    }

    #[test]
    fn inline_matches_ord() {
        assert_cmp(&[0u8, 1, 7, 42, u8::MAX]);
        assert_cmp(&[0u64, 1, 7, u64::MAX]);
        assert_cmp(&[0i64, -1, 1, i64::MIN, i64::MAX]);
        assert_cmp(&[0usize, usize::MAX]);
        assert_cmp(&['a', 'A', '\0', 'α', char::MAX]);
        //inline types never touch the heap
        let mut b = Block::<u32>::new(64);
        {
            let mut al = b.allocator();
            u64::archive(&5, &mut al).unwrap();
        }
        assert_eq!(b.heap_len(), 0);
    }

    ///7/8/9-byte cutoff family + prefix collisions (first 8 bytes equal,
    ///differ later) + embedded NULs — the prefix path must never disagree
    ///with the full compare
    #[test]
    fn string_family_matches_ord() {
        let strs: Vec<String> = [
            "", "a", "ab", "abc", "abcdefg", "abcdefgh", "abcdefghi",
            "abcdefghij", "abcdefghz", "abcdefgh\0", "z", "αβ", "αβγ",
            "héllo", "hello", "héllo wörld", "hello worle", "hello world",
            "ab\0\0\0\0\0\0cd", "ab\0x", "ab\0\0y",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        assert_cmp(&strs);
    }

    #[test]
    fn string_layout() {
        let mut b = Block::<u32>::new(128);
        let inlines = ["", "abc", "abcdefgh"];
        let hs: Vec<StrHandle<u32>> = {
            let mut al = b.allocator();
            inlines.iter().map(|s| String::archive(&s.to_string(), &mut al).unwrap()).collect()
        };
        assert_eq!(b.heap_len(), 0, "≤8-byte strings stay inline");
        for (s, h) in inlines.iter().zip(&hs) {
            assert_eq!(h.h.0.as_usize(), 0, "inline marker");
            assert_eq!(h.h.1.as_usize(), s.len());
            assert_eq!(h.pfx, prefix64(s.as_bytes()));
        }
        {
            let mut al = b.allocator();
            String::archive(&"abcdefghi".to_string(), &mut al).unwrap();
        }
        assert_eq!(b.heap_len(), 9);
    }

    #[test]
    fn vec_u32_flat() {
        let mut b = Block::<u32>::new(128);
        let empty = {
            let mut al = b.allocator();
            Vec::<u32>::archive(&vec![], &mut al).unwrap()
        };
        assert_eq!(empty.h, H(0u32, 0u32));
        assert_eq!(empty.len.as_usize(), 0);
        assert_eq!(b.heap_len(), 0, "empty vec allocates nothing");
        assert_cmp(&[vec![], vec![0], vec![1], vec![1, 2], vec![1, 2, 3],
            vec![1, 2, 0], vec![1, 2, 2], vec![2], vec![u32::MAX],
            vec![1, u32::MAX]]);
        let three = {
            let mut al = b.allocator();
            Vec::<u32>::archive(&vec![1, 2, 3], &mut al).unwrap()
        };
        //u8 vec: byte-exact raw region
        let byts = {
            let mut al = b.allocator();
            Vec::<u8>::archive(&vec![9, 7, 5], &mut al).unwrap()
        };
        assert_eq!(byts.len.as_usize(), 3);
        //ONE region of raw handles: H back-relative, aligned for T
        let heap = Heap::new(&b);
        assert_eq!(three.h.1.as_usize(), 12);
        for (i, v) in [1u32, 2, 3].iter().enumerate() {
            assert_eq!(*heap.handle::<u32>(three.h, i), *v);
        }
        assert_eq!(heap.bytes(byts.h), &[9, 7, 5]);
    }

    #[test]
    fn vec_string_matches_ord() {
        let arr: [Vec<String>; 7] = [
            vec![],
            vec!["".into()],
            vec!["a".into(), "b".into()],
            vec!["abcdefghi".into(), "abc".into()],
            vec!["abcdefghi".into(), "abd".into()],
            vec!["abcdefghj".into()],
            vec!["b".into()],
        ];
        let vs: Vec<Vec<String>> = arr.into_iter().collect();
        assert_cmp(&vs);
        //handles region really holds StrHandles, each inline element with no
        //heap bytes of its own
        let mut b = Block::<u32>::new(256);
        let k = vec!["a".to_string(), "bcdefghijk".to_string()];
        let h = {
            let mut al = b.allocator();
            Vec::<String>::archive(&k, &mut al).unwrap()
        };
        let heap = Heap::new(&b);
        assert_eq!(h.len.as_usize(), 2);
        let e0 = heap.handle::<StrHandle<u32>>(h.h, 0);
        assert_eq!(e0.h.0.as_usize(), 0, "inline element");
        let e1 = heap.handle::<StrHandle<u32>>(h.h, 1);
        assert_eq!(heap.bytes(e1.h), b"bcdefghijk");
    }

    ///the higher-order shape that proves derive-feasibility: outer handles
    ///region holds inner VecH handles, each naming its own regions
    #[test]
    fn vec_vec_string_matches_ord() {
        let arr: [Vec<Vec<String>>; 6] = [
            vec![vec![]],
            vec![vec!["a".into()], vec!["b".into()]],
            vec![vec!["a".into(), "b".into()], vec![]],
            vec![vec!["abcdefghi".into()], vec!["z".into(), "a".into()]],
            vec![vec!["abcdefghi".into()], vec!["za".into()]],
            vec![vec!["abcdefghi".into()], vec![]],
        ];
        let vs: Vec<Vec<Vec<String>>> = arr.into_iter().collect();
        assert_cmp(&vs);
    }

    ///drop recursion: children (each string region, then its inner handles
    ///region) before the region naming them — every region exactly once
    #[test]
    fn nested_drop_releases_every_region_once() {
        let mut b = Block::<u32>::new(256);
        let outer = vec![
            vec!["abcdefghi".to_string()],
            vec!["bcdefghijk".to_string(), "cdefghijk".to_string()],
        ];
        let oh = {
            let mut al = b.allocator();
            Vec::<Vec<String>>::archive(&outer, &mut al).unwrap()
        };
        let expected = {
            let mut al = b.allocator();
            let i0 = *al.handle::<VecH<u32>>(oh.h, 0);
            let i1 = *al.handle::<VecH<u32>>(oh.h, 1);
            let s0 = *al.handle::<StrHandle<u32>>(i0.h, 0);
            let s1 = *al.handle::<StrHandle<u32>>(i1.h, 0);
            let s2 = *al.handle::<StrHandle<u32>>(i1.h, 1);
            let e = [s0.h, i0.h, s1.h, s2.h, i1.h, oh.h];
            <Vec<Vec<String>> as Archive<u32>>::release(&oh, &mut al);
            e
        };
        assert_eq!(b.freed(), &expected);
    }

    ///flat vec frees its one region; inline string handles name nothing
    #[test]
    fn flat_drop_and_inline_noop() {
        let mut b = Block::<u32>::new(256);
        let vh = {
            let mut al = b.allocator();
            Vec::<u32>::archive(&vec![1, 2, 3], &mut al).unwrap()
        };
        let sh = {
            let mut al = b.allocator();
            String::archive(&"short".to_string(), &mut al).unwrap()
        };
        {
            let mut al = b.allocator();
            <Vec<u32> as Archive<u32>>::release(&vh, &mut al);
            <String as Archive<u32>>::release(&sh, &mut al);
        }
        assert_eq!(b.freed(), &[vh.h]);
    }

    #[test]
    fn narrow_pointer_width() {
        let mut b = Block::<u16>::new(256);
        let strs = ["abcdefghi".to_string(), "abcdefgh".to_string()];
        let hs: Vec<StrHandle<u16>> = {
            let mut al = b.allocator();
            strs.iter().map(|s| String::archive(s, &mut al).unwrap()).collect()
        };
        let heap = Heap::new(&b);
        assert_eq!(String::rel_cmp(&hs[0], &strs[1], &heap), Ordering::Greater);
        assert_eq!(String::rel_cmp(&hs[1], &strs[0], &heap), Ordering::Less);
        assert_eq!(String::rel_cmp_h(&hs[0], &hs[1], &heap), Ordering::Greater);
        assert_eq!(heap.bytes(hs[0].h), b"abcdefghi");
        assert_eq!(hs[1].h.0.as_usize(), 0);
    }

    ///a failed archive must leave heap_len and free identical — the mid-loop
    ///failure allocates the handles region + some strings before dying
    #[test]
    fn failed_archive_leaves_heap_unchanged() {
        let mut b = Block::<u32>::new(128);
        let strs: Vec<String> =
            (0..6).map(|i| format!("string-{i}-padding")).collect();
        let (heap0, free0) = (b.heap_len(), b.free_bytes());
        let r = { let mut al = b.allocator(); Vec::<String>::archive(&strs, &mut al) };
        assert!(r.is_err());
        assert_eq!(b.heap_len(), heap0);
        assert_eq!(b.free_bytes(), free0);
        //rewound space is reusable
        {
            let mut al = b.allocator();
            String::archive(&"twelve_bytes".to_string(), &mut al).unwrap();
        }
        assert_eq!(b.heap_len(), 12);
    }

    #[test]
    #[should_panic(expected = "rewind ahead")]
    fn rewind_ahead_panics() {
        let mut b = Block::<u32>::new(64);
        let mut al = b.allocator();
        let mut s = al.snapshot();
        s.heap += 1;
        al.rewind(s);
    }

    ///single-alloc archives are atomic by construction; pin it at the cutoff
    #[test]
    fn failed_string_alloc_atomic() {
        let mut b = Block::<u32>::new(64);
        let (heap0, free0) = (b.heap_len(), b.free_bytes());
        let r = {
            let mut al = b.allocator();
            String::archive(
                &"way-past-cap-string_____________________________________________________________________".to_string(),
                &mut al,
            )
        };
        assert!(r.is_err());
        assert_eq!(b.heap_len(), heap0);
        assert_eq!(b.free_bytes(), free0);
    }

    ///explicit-pad size pins: StrHandle normalizes via P::TailPad2 (16 at
    ///widths ≤ 4 bytes, 24 at 8), VecH via P::TailPad3 (8/8/16/24) — both
    ///8-mult at every width so composite boundaries stay width-independent
    #[test]
    fn handle_pad_size_pins() {
        use std::mem::{align_of, size_of};
        assert_eq!(size_of::<StrHandle<u8>>(), 16);
        assert_eq!(size_of::<StrHandle<u16>>(), 16);
        assert_eq!(size_of::<StrHandle<u32>>(), 16);
        assert_eq!(size_of::<StrHandle<u64>>(), 24);
        assert_eq!(size_of::<StrHandle<usize>>(), 24);
        assert_eq!(align_of::<StrHandle<u32>>(), 8);
        assert_eq!(size_of::<VecH<u8>>(), 8);
        assert_eq!(size_of::<VecH<u16>>(), 8);
        assert_eq!(size_of::<VecH<u32>>(), 16);
        assert_eq!(size_of::<VecH<u64>>(), 24);
        assert_eq!(size_of::<VecH<usize>>(), 24);
        assert_eq!(align_of::<VecH<u16>>(), 2);
    }

    ///narrow-width record hole closed: handles whose widths carry pads ride
    ///records with every pad byte DEFINED and zero
    #[test]
    fn handle_pads_zero_in_records() {
        let mut b = Block::<u16>::new(64);
        let sh = {
            let mut al = b.allocator();
            String::archive(&"abc".to_string(), &mut al).unwrap()
        };
        b.push::<StrHandle<u16>>(sh).unwrap();
        let rec = b.stack_bytes();
        assert_eq!(rec.len(), 16);
        //pfx rides as a BE-LOADED u64: LE record bytes are zero-high-first,
        //the prefix's bytes reversed at the tail of the word
        assert_eq!(&rec[..5], &[0; 5]);
        assert_eq!(&rec[5..8], b"cba");
        assert_eq!(&rec[8..12], &[0, 0, 3, 0], "inline marker h = H(0, len)");
        assert_eq!(&rec[12..16], &[0; 4], "StrHandle<u16> tail pad");
        let vh = {
            let mut al = b.allocator();
            Vec::<u8>::archive(&vec![9u8, 7, 5], &mut al).unwrap()
        };
        b.push::<VecH<u16>>(vh).unwrap();
        let rec = b.stack_bytes();
        let heap = Heap::new(&b);
        assert_eq!(heap.bytes(vh.h), &[9, 7, 5]);
        assert_eq!(&rec[16..20], &[3, 0, 3, 0], "region H(3, 3)");
        assert_eq!(&rec[20..22], &[3, 0], "len 3");
        assert_eq!(&rec[22..24], &[0; 2], "VecH<u16> tail pad");
    }
}

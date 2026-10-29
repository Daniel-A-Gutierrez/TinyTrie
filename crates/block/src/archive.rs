//!archival + comparison over the block seam: `Archive` shoves K into the
//!block's heap through `Allocator` (the write seam) and returns a fixed-size
//!POD `Handle`; `RelOrd` compares a stored handle against a native query
//!FIELD-WISE through `Heap` (the read seam), short-circuiting at the first
//!unequal field — no view construction on the compare path. no `Ord` bound
//!lives on either trait; primitive impls carry the order, composites zip
//!fields through per-field RelOrd.
//!invariants:
//!- a FAILED archive unwinds itself: every single allocation is atomic by
//!  construction, so multi-alloc impls release the children they built so
//!  far (children first — the partial handle plus the write count IS the
//!  undo log) and roll the heap front back to the entry position
//!  (`Allocator::rollback`). archive impls never free on the success path —
//!  the unwind protocol is the only freeer inside archive.
//!- handles are plain POD with EXPLICIT padding only (ZeroPad + ReprC): every
//!  byte defined, pad bytes zero-written — pushed records never carry
//!  undefined bytes
//!- `A::HEAP == false` ⇒ the handle IS the value; Vec<A> lays the element
//!  handles into ONE region via `archive_slice`, which recurses down to
//!  primitive element archives
//!- `Archive::release` releases every region a handle transitively names,
//!  children first; exclusive ownership, release-once — it is both the
//!  consumer-facing delete and the archive failure unwind
//!- strings ≤ 8 bytes ride the prefix entirely (h.off == 0 = inline); heap
//!  regions satisfy the block's len ≤ off form, so the inline form
//!  (h.len = length) fails the heap canary if it ever reaches `Heap::bytes`

use std::cmp::Ordering;
use std::mem::{MaybeUninit, size_of};

use crate::block::{Allocator, Block, CapErr};
use crate::handle::{H, Pad, ReprC, ZeroPad};

///heap READ seam: handle→region lookups only — a find path needs only &Block
///to build one; release goes through Allocator (the write seam).
pub struct Heap<'a> {
    block: &'a Block,
}

impl<'a> Heap<'a> {
    pub fn new(block: &'a Block) -> Self {
        Heap { block }
    }

    ///region bytes; panics on malformed or stale handles.
    #[inline]
    pub fn bytes(&self, h: H<u8>) -> &[u8] {
        self.block.heap(h)
    }

    ///region as T slots — uninitialized view; only archive-written slots may
    ///be assumed init.
    #[inline]
    pub fn slots<T>(&self, h: H<T>) -> &[MaybeUninit<T>] {
        self.block.heap_uninit(h)
    }

    ///slot `i` of a T region as initialized — caller guarantees archive wrote
    ///a valid T there (handles are plain POD).
    #[inline]
    pub fn slot<T: Copy>(&self, h: H<T>, i: usize) -> &T {
        // SAFETY: caller contract — archive wrote a valid T at slot i
        unsafe { self.slots(h)[i].assume_init_ref() }
    }
}

///storage only: shove K into the block, get a handle. no Ord bound, no cmp.
pub trait Archive: Sized {
    ///fixed-size slot representation; plain POD with EXPLICIT padding only —
    ///every byte defined (ZeroPad + ReprC), padded bytes zero-written.
    ///`Eq` structural.
    type Handle: Copy + Eq + ZeroPad + ReprC;
    ///false = the handle IS the value; the heap is never touched.
    const HEAP: bool;

    ///appends k's heap regions via `al`, returns the handle; on Err the heap
    ///is unchanged.
    fn archive(k: &Self, al: &mut Allocator<'_>) -> Result<Self::Handle, CapErr>;

    ///releases every heap region the handle transitively names — children
    ///before the region naming them; no-op when HEAP is false. the handle
    ///owns its transitive regions EXCLUSIVELY (no sharing): release-once,
    ///releasing twice is a caller bug.
    fn release(h: &Self::Handle, al: &mut Allocator<'_>);
}

///field-wise comparison; every branch short-circuits at the first unequal
///field and touches only the fields it reads.
pub trait RelOrd: Archive {
    ///stored handle vs native k — the tier find order.
    fn rel_cmp(h: &Self::Handle, k: &Self, heap: &Heap<'_>) -> Ordering;
    ///handle vs handle over the shared heap.
    fn rel_cmp_h(a: &Self::Handle, b: &Self::Handle, heap: &Heap<'_>) -> Ordering;
}

impl Allocator<'_> {
    ///the recursive slice primitive: allocates the handles region, then
    ///archives each element and writes its handle into the next slot as it
    ///lands (back-relative H survives the element regions growing below).
    ///element impls that call this again recurse down to the primitives.
    ///atomicity is UNWIND-based, not snapshot-based: every single alloc is
    ///atomic by construction, so a failure releases the children built so
    ///far (children first — the partial handle plus the write count IS the
    ///undo log) and rolls the heap front back to the entry position; the
    ///freed regions die wholesale with the roll.
    pub fn archive_slice<T: Archive>(&mut self, ks: &[T]) -> Result<H<T::Handle>, CapErr> {
        if ks.is_empty() {
            return Ok(H::new(0, 0));
        }
        let heap0 = self.heap_len();
        let rh = match self.alloc::<T::Handle>(ks.len()) {
            Ok(h) => h,
            Err(e) => return Err(e),
        };
        for (i, k) in ks.iter().enumerate() {
            let eh = match T::archive(k, self) {
                Ok(h) => h,
                Err(e) => {
                    self.release_slice::<T>(rh, i);
                    self.rollback(heap0);
                    return Err(e);
                }
            };
            self.write_slot(rh, i, eh);
        }
        Ok(rh)
    }

    ///release of a slice handle: children first (each slot's handle, in
    ///order), then the region itself — the symmetric inverse of
    ///archive_slice and the unwind workhorse. `n` = slots written (the full
    ///count for a complete handle).
    pub fn release_slice<T: Archive>(&mut self, h: H<T::Handle>, n: usize) {
        if size_of::<T::Handle>() == 0 {
            //zero-size handles name no slots; the region is H(0, 0)
            return;
        }
        for i in 0..n {
            let eh = *self.slot(h, i);
            T::release(&eh, self);
        }
        self.free(h);
    }
}

///first ≤8 bytes zero-padded, BE-loaded — the inline compare form (integer
///compare == byte-order compare).
#[inline]
fn str_prefix_64(bytes: &[u8]) -> u64 {
    let mut buf = [0u8; 8];
    let n = bytes.len().min(8);
    buf[..n].copy_from_slice(&bytes[..n]);
    u64::from_be_bytes(buf)
}

///string handle: BE prefix + heap range. h.off == 0 marks INLINE (≤8 bytes —
///the whole string rides pfx, h.len = length); the block's len ≤ off region
///form makes an inline handle fail the heap canary. heap handles carry the
///full bytes; h.len = full length both ways (cmp needs it in-line).
///`_pad` = explicit trailing pad — zero in every record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct StrHandle {
    pub pfx: u64,
    pub h:   H<u8>,
    _pad:    Pad<4>,
}

/// SAFETY: explicit pad field; every byte is a field.
unsafe impl ZeroPad for StrHandle {}
/// SAFETY: declared #[repr(C)] above.
unsafe impl ReprC for StrHandle {}

impl Archive for String {
    type Handle = StrHandle;
    const HEAP: bool = true;

    fn archive(k: &Self, al: &mut Allocator<'_>) -> Result<Self::Handle, CapErr> {
        let bytes = k.as_bytes();
        let len = bytes.len();
        let pfx = str_prefix_64(bytes);
        //short-string cutoff: the whole key rides pfx, no heap bytes
        if len <= 8 {
            return Ok(StrHandle { pfx, h: H::new(0, len as u16), _pad: Default::default() });
        }
        let h = al.alloc_slice::<u8>(bytes)?;
        Ok(StrHandle { pfx, h, _pad: Default::default() })
    }

    fn release(h: &Self::Handle, al: &mut Allocator<'_>) {
        //inline handles name no region (h.off == 0 marker)
        if h.h.off() != 0 {
            al.free(h.h);
        }
    }
}

impl RelOrd for String {
    fn rel_cmp(h: &Self::Handle, k: &Self, heap: &Heap<'_>) -> Ordering {
        let kb = k.as_bytes();
        match h.pfx.cmp(&str_prefix_64(kb)) {
            //pfx-equal: inline h is a proper prefix of (or equal-pad-vs) the
            //query — length decides
            Ordering::Equal if h.h.off() == 0 => (h.h.len() as usize).cmp(&kb.len()),
            Ordering::Equal => heap.bytes(h.h).cmp(kb),
            ord => ord,
        }
    }

    fn rel_cmp_h(a: &Self::Handle, b: &Self::Handle, heap: &Heap<'_>) -> Ordering {
        match a.pfx.cmp(&b.pfx) {
            Ordering::Equal => match (a.h.off() == 0, b.h.off() == 0) {
                //prefix-equal inline handles order by length
                (true, true) => a.h.len().cmp(&b.h.len()),
                //pfx-equal inline is a prefix of the heap string — always <
                (true, false) => Ordering::Less,
                (false, true) => Ordering::Greater,
                (false, false) => heap.bytes(a.h).cmp(heap.bytes(b.h)),
            },
            ord => ord,
        }
    }
}

impl Archive for char {
    type Handle = char;
    const HEAP: bool = false;

    fn archive(k: &Self, _al: &mut Allocator<'_>) -> Result<Self::Handle, CapErr> {
        Ok(*k)
    }

    fn release(_h: &Self::Handle, _al: &mut Allocator<'_>) {}
}

impl RelOrd for char {
    fn rel_cmp(h: &Self::Handle, k: &Self, _heap: &Heap<'_>) -> Ordering {
        h.cmp(k)
    }

    fn rel_cmp_h(a: &Self::Handle, b: &Self::Handle, _heap: &Heap<'_>) -> Ordering {
        a.cmp(b)
    }
}

///vec handle: `h` names the handles region, `len` the element count (stored
///separately — the region may be NODE-rounded past the exact byte count).
///`_pad` keeps the handle 8-mult so derived composites align boundaries
///without width-dependent gaps.
#[repr(C)]
pub struct VecHandle<A: Archive> {
    pub h:   H<A::Handle>,
    pub len: u16,
    _pad:    Pad<2>,
}

//hand-written: derives would bound the impls on A itself, but only A::Handle
//rides the struct
impl<A: Archive> Clone for VecHandle<A> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<A: Archive> Copy for VecHandle<A> {}
impl<A: Archive> PartialEq for VecHandle<A> {
    fn eq(&self, other: &Self) -> bool {
        self.h == other.h && self.len == other.len && self._pad == other._pad
    }
}
impl<A: Archive> Eq for VecHandle<A> {}
impl<A: Archive> std::fmt::Debug for VecHandle<A> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        //raw ints — A::Handle carries no Debug bound
        f.debug_struct("VecHandle").field("off", &self.h.off()).field("len", &self.len).finish()
    }
}

/// SAFETY: explicit pad field; every byte is a field.
unsafe impl<A: Archive> ZeroPad for VecHandle<A> {}
/// SAFETY: declared #[repr(C)] above.
unsafe impl<A: Archive> ReprC for VecHandle<A> {}

impl<A: Archive> Archive for Vec<A> {
    type Handle = VecHandle<A>;
    const HEAP: bool = true;

    fn archive(k: &Self, al: &mut Allocator<'_>) -> Result<Self::Handle, CapErr> {
        //the count rides a u16 — more elements than the width can name is a
        //terminal Err, never a silent truncation
        if k.len() > u16::MAX as usize {
            return Err(CapErr { free: al.free_bytes() });
        }
        let h = al.archive_slice(k)?;
        Ok(VecHandle { h, len: k.len() as u16, _pad: Default::default() })
    }

    fn release(h: &Self::Handle, al: &mut Allocator<'_>) {
        al.release_slice::<A>(h.h, h.len as usize);
    }
}

impl<A: RelOrd> RelOrd for Vec<A> {
    fn rel_cmp(h: &Self::Handle, k: &Self, heap: &Heap<'_>) -> Ordering {
        if size_of::<A::Handle>() == 0 {
            //zero-size handles carry no data — the count is the whole value
            return (h.len as usize).cmp(&k.len());
        }
        let n = (h.len as usize).min(k.len());
        for i in 0..n {
            let eh = heap.slot(h.h, i);
            let ord = A::rel_cmp(eh, &k[i], heap);
            if ord != Ordering::Equal {
                return ord;
            }
        }
        (h.len as usize).cmp(&k.len())
    }

    fn rel_cmp_h(a: &Self::Handle, b: &Self::Handle, heap: &Heap<'_>) -> Ordering {
        if size_of::<A::Handle>() == 0 {
            return (a.len as usize).cmp(&(b.len as usize));
        }
        let n = (a.len as usize).min(b.len as usize);
        for i in 0..n {
            let ord = A::rel_cmp_h(heap.slot(a.h, i), heap.slot(b.h, i), heap);
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
        impl Archive for $t {
            type Handle = $t;
            const HEAP: bool = false;
            #[inline]
            fn archive(k: &Self, _al: &mut Allocator<'_>) -> Result<Self::Handle, CapErr> {
                Ok(*k)
            }
            #[inline]
            fn release(_h: &Self::Handle, _al: &mut Allocator<'_>) {}
        }
        impl RelOrd for $t {
            #[inline]
            fn rel_cmp(h: &Self::Handle, k: &Self, _heap: &Heap<'_>) -> Ordering { h.cmp(k) }
            #[inline]
            fn rel_cmp_h(a: &Self::Handle, b: &Self::Handle, _heap: &Heap<'_>) -> Ordering {
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
    fn assert_cmp<K: RelOrd + Ord>(ks: &[K]) {
        let mut b = Block::new(65280);
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
        let mut b = Block::new(64);
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
            "",
            "a",
            "ab",
            "abc",
            "abcdefg",
            "abcdefgh",
            "abcdefghi",
            "abcdefghij",
            "abcdefghz",
            "abcdefgh\0",
            "z",
            "αβ",
            "αβγ",
            "héllo",
            "hello",
            "héllo wörld",
            "hello worle",
            "hello world",
            "ab\0\0\0\0\0\0cd",
            "ab\0x",
            "ab\0\0y",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        assert_cmp(&strs);
    }

    #[test]
    fn string_layout() {
        let mut b = Block::new(128);
        let inlines = ["", "abc", "abcdefgh"];
        let hs: Vec<StrHandle> = {
            let mut al = b.allocator();
            inlines.iter().map(|s| String::archive(&s.to_string(), &mut al).unwrap()).collect()
        };
        assert_eq!(b.heap_len(), 0, "≤8-byte strings stay inline");
        for (s, h) in inlines.iter().zip(&hs) {
            assert_eq!(h.h.off(), 0, "inline marker");
            assert_eq!(h.h.len() as usize, s.len());
            assert_eq!(h.pfx, str_prefix_64(s.as_bytes()));
        }
        {
            let mut al = b.allocator();
            String::archive(&"abcdefghi".to_string(), &mut al).unwrap();
        }
        assert_eq!(b.heap_len(), 9);
    }

    #[test]
    fn vec_u32_flat() {
        let mut b = Block::new(128);
        let empty = {
            let mut al = b.allocator();
            Vec::<u32>::archive(&vec![], &mut al).unwrap()
        };
        assert_eq!((empty.h.off(), empty.h.len()), (0, 0));
        assert_eq!(empty.len, 0);
        assert_eq!(b.heap_len(), 0, "empty vec allocates nothing");
        assert_cmp(&[
            vec![],
            vec![0],
            vec![1],
            vec![1, 2],
            vec![1, 2, 3],
            vec![1, 2, 0],
            vec![1, 2, 2],
            vec![2],
            vec![u32::MAX],
            vec![1, u32::MAX],
        ]);
        let three = {
            let mut al = b.allocator();
            Vec::<u32>::archive(&vec![1, 2, 3], &mut al).unwrap()
        };
        //u8 vec: byte-exact raw region (NODE-rounded to 4)
        let byts = {
            let mut al = b.allocator();
            Vec::<u8>::archive(&vec![9, 7, 5], &mut al).unwrap()
        };
        assert_eq!(byts.len as usize, 3);
        //ONE region of raw handles
        let heap = Heap::new(&b);
        assert_eq!(three.h.len() as usize, 12);
        for (i, v) in [1u32, 2, 3].iter().enumerate() {
            assert_eq!(*heap.slot(three.h, i), *v);
        }
        assert_eq!(heap.bytes(byts.h), &[9, 7, 5, 0, 0, 0], "span 6 = NODE + 3×u8");
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
        let mut b = Block::new(256);
        let k = vec!["a".to_string(), "bcdefghijk".to_string()];
        let h = {
            let mut al = b.allocator();
            Vec::<String>::archive(&k, &mut al).unwrap()
        };
        let heap = Heap::new(&b);
        assert_eq!(h.len as usize, 2);
        let e0 = *heap.slot::<StrHandle>(h.h, 0);
        assert_eq!(e0.h.off(), 0, "inline element");
        let e1 = *heap.slot::<StrHandle>(h.h, 1);
        assert_eq!(heap.bytes(e1.h), b"bcdefghijk");
    }

    ///the higher-order shape that proves derive-feasibility: outer handles
    ///region holds inner VecHandles, each naming its own regions
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
        let mut b = Block::new(256);
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
            let i0 = *al.slot::<VecHandle<String>>(oh.h, 0);
            let i1 = *al.slot::<VecHandle<String>>(oh.h, 1);
            let s0 = *al.slot::<StrHandle>(i0.h, 0);
            let s1 = *al.slot::<StrHandle>(i1.h, 0);
            let s2 = *al.slot::<StrHandle>(i1.h, 1);
            let e = [
                (s0.h.off(), s0.h.len()),
                (i0.h.off(), i0.h.len()),
                (s1.h.off(), s1.h.len()),
                (s2.h.off(), s2.h.len()),
                (i1.h.off(), i1.h.len()),
                (oh.h.off(), oh.h.len()),
            ];
            <Vec<Vec<String>> as Archive>::release(&oh, &mut al);
            e
        };
        assert_eq!(b.freed(), &expected);
    }

    ///flat vec frees its one region; inline string handles name nothing
    #[test]
    fn flat_drop_and_inline_noop() {
        let mut b = Block::new(256);
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
            <Vec<u32> as Archive>::release(&vh, &mut al);
            <String as Archive>::release(&sh, &mut al);
        }
        assert_eq!(b.freed(), &[(vh.h.off(), vh.h.len())]);
    }

    #[test]
    fn handle_pad_size_pins() {
        use std::mem::{align_of, size_of};
        assert_eq!(size_of::<StrHandle>(), 16);
        assert_eq!(align_of::<StrHandle>(), 8);
        assert_eq!(size_of::<VecHandle<u32>>(), 8);
        assert_eq!(align_of::<VecHandle<u32>>(), 2);
        assert_eq!(size_of::<VecHandle<String>>(), 8);
        assert_eq!(size_of::<VecHandle<Vec<String>>>(), 8);
    }

    ///pads ride records zero: an archived StrHandle pushed onto the stack
    ///reads zero at its pad bytes, round-trips byte-exact
    #[test]
    fn handle_pads_zero_in_records() {
        let mut b = Block::new(64);
        let sh = {
            let mut al = b.allocator();
            String::archive(&"abc".to_string(), &mut al).unwrap()
        };
        b.push::<StrHandle>(sh).unwrap();
        let rec = b.stack_bytes();
        assert_eq!(rec.len(), 16);
        //pfx rides as a BE-LOADED u64: LE record bytes are zero-high-first,
        //the prefix's bytes reversed at the tail of the word
        assert_eq!(&rec[..5], &[0; 5]);
        assert_eq!(&rec[5..8], b"cba");
        assert_eq!(&rec[8..12], &[0, 0, 3, 0], "inline marker h = H(0, len)");
        assert_eq!(&rec[12..16], &[0; 4], "StrHandle tail pad");
        let vh = {
            let mut al = b.allocator();
            Vec::<u8>::archive(&vec![9u8, 7, 5], &mut al).unwrap()
        };
        b.push::<VecHandle<u8>>(vh).unwrap();
        let rec = b.stack_bytes();
        let heap = Heap::new(&b);
        assert_eq!(heap.bytes(vh.h), &[9, 7, 5, 0, 0, 0]);
        assert_eq!(&rec[16..20], &[10, 0, 6, 0], "region H(10, 6) — NODE-rounded");
        assert_eq!(&rec[20..22], &[3, 0], "len 3");
        assert_eq!(&rec[22..24], &[0; 2], "VecHandle tail pad");
    }

    ///a failed archive must leave heap_len and free identical — the mid-loop
    ///failure allocates the handles region + some strings before dying
    #[test]
    fn failed_archive_leaves_heap_unchanged() {
        let mut b = Block::new(128);
        let strs: Vec<String> = (0..6).map(|i| format!("string-{i}-padding")).collect();
        let (heap0, free0) = (b.heap_len(), b.free_bytes());
        let r = {
            let mut al = b.allocator();
            Vec::<String>::archive(&strs, &mut al)
        };
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

    ///single-alloc archives are atomic by construction; pin it at the cutoff
    #[test]
    fn failed_string_alloc_atomic() {
        let mut b = Block::new(64);
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
}

//!archivable key/value forms: a stored K decomposes into a fixed-size
//!`Handle` in the block's slots + bytes in an append-only side heap.
//!inline types (integers, char) are their own handles and never touch the
//!heap. handle offsets are heap-absolute (u32 — heap capped at 4 GiB) and
//!endian-normalized (LE), so the (slots, heap) pair IS the serialized form
//!and handles survive slot relocation (slides) and whole-buffer moves.
//!composites (derive, future): handle points at the allocation's start;
//!field offsets inside the allocation are allocation-relative.
//!invariants:
//!- `cmp` on any stored handle equals `Ord` on the archived value — the
//!  set's total order is the key's own order
//!- the heap never reorders or reclaims (v1): removal leaks dead bytes; a
//!  later vacuum compacts — offsets are internal, never held outside the
//!  block, so compaction needs no fixups

use std::cmp::Ordering;

///key/value form stored in slots: archive appends heap bytes and returns
///the handle; cmp orders a stored handle against a query key; view reads
///back. Handle is Copy, owns nothing, serializes as-is.
pub trait Archive: Sized + Ord {
    ///fixed-size slot representation; POD
    type Handle: Copy;
    ///borrowed read form over heap bytes
    type View<'h>;

    ///append k's heap bytes, return its handle. panics past a 4 GiB heap
    fn archive(k: &Self, heap: &mut Vec<u8>) -> Self::Handle;
    ///order of a stored handle vs a query key — equals `Ord` on the pair
    fn cmp_key(h: &Self::Handle, k: &Self, heap: &[u8]) -> Ordering;
    ///read back. panics on a corrupt or out-of-range handle
    fn view<'h>(h: &'h Self::Handle, heap: &'h [u8]) -> Self::View<'h>;
    ///reconstruct the owned value from a spent handle. panics on a corrupt
    ///or out-of-range handle
    fn materialize(h: Self::Handle, heap: &[u8]) -> Self;
}

///String handle: 8 zero-padded inline bytes + heap range. strings ≤ 8
///bytes live ENTIRELY in `pfx` (len = the inline length, off unused, no
///heap bytes); longer strings store their full bytes in the heap (`pfx`
///replicated — view needs one contiguous run).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StrHandle {
    ///zero-padded first bytes; the whole string when len ≤ 8
    pfx: [u8; 8],
    off: u32,
    len: u32,
}

impl StrHandle {
    ///the handle's heap slice (len > 8 only). panics if out of range
    fn bytes<'h>(&self, heap: &'h [u8]) -> &'h [u8] {
        &heap[self.off as usize..self.off as usize + self.len as usize]
    }
}

///first ≤8 bytes zero-padded — the inline compare form
fn prefix(bytes: &[u8]) -> [u8; 8] {
    let mut buf = [0u8; 8];
    let n = bytes.len().min(8);
    buf[..n].copy_from_slice(&bytes[..n]);
    buf
}

impl Archive for String {
    type Handle = StrHandle;
    type View<'h> = &'h str;

    fn archive(k: &Self, heap: &mut Vec<u8>) -> Self::Handle {
        let bytes = k.as_bytes();
        let len = bytes.len();
        //short-string cutoff: the whole key lives inline, no heap bytes
        if len <= 8 {
            return StrHandle { pfx: prefix(bytes), off: 0, len: len as u32 };
        }
        let off = u32::try_from(heap.len()).expect("heap overflows u32");
        heap.extend_from_slice(bytes);
        StrHandle { pfx: prefix(bytes), off, len: len as u32 }
    }

    fn cmp_key(h: &Self::Handle, k: &Self, heap: &[u8]) -> Ordering {
        match h.pfx.cmp(&prefix(k.as_bytes())) {
            //inline: pfx equal ⇒ identical, or h.len == 8 < k.len (h is
            //k's prefix) — length decides; no heap bytes exist
            Ordering::Equal if h.len <= 8 => h.len.cmp(&(k.len() as u32)),
            //heap: prefix-equal ⇒ both ≥ 8 — the heap compare resolves
            Ordering::Equal => h.bytes(heap).cmp(k.as_bytes()),
            ord => ord,
        }
    }

    fn view<'h>(h: &'h Self::Handle, heap: &'h [u8]) -> Self::View<'h> {
        // SAFETY (both arms): archive writes only `String` bytes — valid
        // utf-8 by construction, and the heap never mutates after append
        unsafe {
            if h.len <= 8 {
                str::from_utf8_unchecked(&h.pfx[..h.len as usize])
            } else {
                str::from_utf8_unchecked(h.bytes(heap))
            }
        }
    }

    fn materialize(h: Self::Handle, heap: &[u8]) -> Self {
        //re-checked on the read path — a free corruption tripwire
        str::from_utf8(Self::view(&h, heap).as_bytes())
            .expect("stored bytes are valid utf-8")
            .to_owned()
    }
}

///heap byte range — Vec<u32>'s handle
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    off: u32,
    end: u32,
}

impl Span {
    ///the span's heap slice. panics if out of range
    fn bytes<'h>(&self, heap: &'h [u8]) -> &'h [u8] {
        &heap[self.off as usize..self.end as usize]
    }
}

///LE u32 stream over a heap byte range — Vec<u32>'s View
pub struct U32s<'h> {
    bytes: &'h [u8],
}

impl Iterator for U32s<'_> {
    type Item = u32;

    #[inline]
    fn next(&mut self) -> Option<u32> {
        let (chunk, rest) = self.bytes.split_first_chunk()?;
        self.bytes = rest;
        Some(u32::from_le_bytes(*chunk))
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.bytes.len() / 4;
        (n, Some(n))
    }
}

impl ExactSizeIterator for U32s<'_> {}

impl Archive for Vec<u32> {
    type Handle = Span;
    type View<'h> = U32s<'h>;

    fn archive(k: &Self, heap: &mut Vec<u8>) -> Self::Handle {
        //4-align: future zero-copy &[u32] views + the derive's packed arrays
        while heap.len() & 3 != 0 {
            heap.push(0);
        }
        let off = u32::try_from(heap.len()).expect("heap overflows u32");
        heap.extend(k.iter().flat_map(|x| x.to_le_bytes()));
        let end = u32::try_from(heap.len()).expect("heap overflows u32");
        Span { off, end }
    }

    fn cmp_key(h: &Self::Handle, k: &Self, heap: &[u8]) -> Ordering {
        U32s { bytes: h.bytes(heap) }.cmp(k.iter().copied())
    }

    fn view<'h>(h: &Self::Handle, heap: &'h [u8]) -> Self::View<'h> {
        U32s { bytes: h.bytes(heap) }
    }

    fn materialize(h: Self::Handle, heap: &[u8]) -> Self {
        U32s { bytes: h.bytes(heap) }.collect()
    }
}

macro_rules! impl_archive_inline {
    ($($t:ty),* $(,)?) => {$(
        impl Archive for $t {
            type Handle = $t;
            type View<'h> = &'h $t;

            fn archive(k: &Self, _heap: &mut Vec<u8>) -> Self::Handle { *k }
            fn cmp_key(h: &Self::Handle, k: &Self, _heap: &[u8]) -> Ordering { h.cmp(k) }
            fn view<'h>(h: &'h Self::Handle, _heap: &'h [u8]) -> Self::View<'h> { h }
            fn materialize(h: Self::Handle, _heap: &[u8]) -> Self { h }
        }
    )*};
}

impl_archive_inline!(u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, i128, isize, char);

#[cfg(test)]
mod tests {
    use super::*;

    ///archive every k into one heap; cmp on every (stored, query) pair
    ///must equal Ord on the values
    fn assert_cmp<K: Archive>(ks: &[K]) {
        let mut heap = Vec::new();
        let hs: Vec<K::Handle> = ks.iter().map(|k| K::archive(k, &mut heap)).collect();
        for (i, a) in ks.iter().enumerate() {
            for (j, b) in ks.iter().enumerate() {
                assert_eq!(K::cmp_key(&hs[i], b, &heap), a.cmp(b), "pair ({i}, {j})");
            }
        }
    }

    #[test]
    fn inline_cmp_matches_ord() {
        assert_cmp(&[0u64, 1, 7, 42, u64::MAX]);
        assert_cmp(&[0u32, 1, u32::MAX]);
        assert_cmp(&[0i64, -1, 1, i64::MIN, i64::MAX]);
        assert_cmp(&['a', 'A', '\0', 'α', char::MAX]);
        //inline types never touch the heap
        let mut heap = Vec::new();
        u64::archive(&5, &mut heap);
        assert!(heap.is_empty());
    }

    ///7/8/9-byte boundary family, prefix-of pairs, multibyte — the prefix
    ///fast path must never disagree with the full compare
    #[test]
    fn string_cmp_matches_ord() {
        let strs: Vec<String> = [
            "", "a", "ab", "abc", "abcdefg", "abcdefgh", "abcdefghi",
            "abcdefghij", "abcdefghz", "z", "αβ", "αβγ", "héllo", "hello",
            "hello world", "hello worle", "héllo wörld",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        assert_cmp(&strs);
        let mut heap = Vec::new();
        for k in &strs {
            let h = String::archive(k, &mut heap);
            assert_eq!(String::view(&h, &heap), k.as_str());
            assert_eq!(String::materialize(h, &heap), *k);
        }
    }

    #[test]
    fn vec_u32_cmp_matches_ord() {
        let vs: Vec<Vec<u32>> = vec![
            vec![],
            vec![1],
            vec![1, 2],
            vec![1, 2, 3],
            vec![2],
            vec![0],
            vec![u32::MAX],
            vec![0x0102_0304],
            vec![1, 2, 0x0102_0304],
        ];
        assert_cmp(&vs);
        let mut heap = Vec::new();
        for k in &vs {
            let h = Vec::<u32>::archive(k, &mut heap);
            assert_eq!(Vec::<u32>::view(&h, &heap).collect::<Vec<_>>(), *k);
            assert_eq!(Vec::<u32>::materialize(h, &heap), *k);
        }
    }

    ///canonical LE layout + the 4-alignment pad after a heap-string insert
    #[test]
    fn vec_u32_layout() {
        let mut heap = Vec::new();
        String::archive(&"twelve_bytes!".to_string(), &mut heap);
        let h = Vec::<u32>::archive(&vec![0x0102_0304], &mut heap);
        assert_eq!(h.off % 4, 0, "packed u32s must be 4-aligned");
        assert_eq!(h.bytes(&heap), &[4, 3, 2, 1]);
        //empty vec: empty span, no bytes
        let e = Vec::<u32>::archive(&vec![], &mut heap);
        assert_eq!(e.bytes(&heap), &[]);
    }

    ///short strings archive fully inline — no heap bytes; round-trip view +
    ///materialize. the 8/9 boundary is the cmp family's job (assert_cmp)
    #[test]
    fn string_short_inline() {
        let mut heap = Vec::new();
        for s in ["", "a", "abc", "abcdefg", "abcdefgh"] {
            let k = s.to_string();
            let h = String::archive(&k, &mut heap);
            assert_eq!(String::view(&h, &heap), s);
            assert_eq!(String::materialize(h, &heap), k);
        }
        assert!(heap.is_empty(), "short strings must not touch the heap");
        //9 bytes crosses the cutoff — heap-stored, full bytes
        let h = String::archive(&"abcdefghi".to_string(), &mut heap);
        assert_eq!(heap, b"abcdefghi");
        assert_eq!(String::view(&h, &heap), "abcdefghi");
    }
}
```rust
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
///L0021
///key/value form stored in slots: archive appends heap bytes and returns
///the handle; cmp orders a stored handle against a query key; view reads
///back. Handle is Copy, owns nothing, serializes as-is.
pub trait Archive: Sized + Ord {
    ///fixed-size slot representation; POD. Eq is structural — borrowed
    ///slots copy a handle verbatim, and real values never repeat
    type Handle: Copy + Eq;
    ///borrowed read form over heap bytes
    type View<'h>;
    ///least handle: the head placeholder for borrowed tier slots. compares
    ///≤ every real handle (cmp_key never Greater)
    const MIN: Self::Handle;
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
///L0047
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
///L0054
impl StrHandle {}
///L0062
///first ≤8 bytes zero-padded — the inline compare form
fn prefix(bytes: &[u8]) -> [u8; 8];
///L0069
impl Archive for String {}
///L0119
///heap byte range — Vec<u32>'s handle
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    off: u32,
    end: u32,
}
///L0124
impl Span {}
///L0132
///LE u32 stream over a heap byte range — Vec<u32>'s View
pub struct U32s<'h> {
    bytes: &'h [u8],
}
///L0136
impl Iterator for U32s<'_> {}
///L0153
impl ExactSizeIterator for U32s<'_> {}
```

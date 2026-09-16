```rust
//!address↔position translation, fn-ptr-specialized over the 16
//!(inner/outer/shift/rotation × zero/nonzero) combos so a steady param is
//!straight-line with no per-lookup branch. `a2p` is the hot path; `p2a` runs
//!on remap.
//!
//!invariant: `p2a(p) = ((p + inner_offset) << shift).ror(rotation) + outer_offset`,
//!and `a2p` is the exact inverse — round-trip exact on canonical
//!(block-handed-out) addrs. addrs may wrap; the only hard rule is position
//!order: pos 0 = min element, pos len−1 = max.
// specialized fn-ptr aliases — the `variant!`/`apply!` macros below generate
// the 16 specialized bodies.
// p2a(p) = ((p + inner_offset) << shift) ror rotation + outer_offset.
// a2p(a) = ((a - outer_offset) rol rotation) >> shift - inner_offset   (exact
// inverse on canonical slots).
// inner_offset lives in position space (added before the shift); outer_offset
// in address space (added after).
// Each op whose param is 0 is a runtime no-op the CPU does NOT elide (see
// bench notes), so specialize picks a pre-baked body that skips zero-param ops
// entirely — straight-line, no per-iter branch, no mispredict risk. Dispatch
// happens once per set_*, not per lookup; the call target is constant for the
// life of the params, so the BTB-predicted indirect call costs ~1 cycle on the
// addr chain (see bench).
///L0027
type A2p<A> = fn(A, A, A, u32, u32) -> A; // x, inner, outer, shift, rotation
///L0028
type P2a<A> = fn(A, A, A, u32, u32) -> A;
// apply x.method(arg) only when the param is nonzero (nz); z is a passthrough.
///L0031
macro_rules! apply;
// generate one a2p/p2a pair for a given (inner, outer, shift, rot) nz/z pattern.
// a2p inverts p2a in reverse op order: ror, sub outer, shr, sub inner.
///L0042
macro_rules! variant;
///L0069
///address↔position translator using fn-ptr specialization (see bench notes /
///a2p_fnptr). `set_*` re-points a2p/p2a when the block's params change
///(grow/spread/graduate). for a statically-known strategy, a const-generic
///block inlines the math and beats even this — Translator is for the adaptive
///tier.
#[derive(Clone)]
pub struct Translator<A> {
    inner_offset: A,
    outer_offset: A,
    shift:        u32,
    rotation:     u32,
    a2p:          A2p<A>,
    p2a:          P2a<A>,
}
///L0080
///address ↔ position translation. `A` is the in-block address type; positions
///are `Pos`. a2p is the hot lookup path, p2a runs on remap.
pub trait AddressTranslator<A>: Sized {
    ///address to position
    fn a2p(&self, addr: A) -> Pos;
    ///position to address
    fn p2a(&self, pos: Pos) -> A;
    ///position-space abs distance between two addrs;
    fn adist(&self, a1: A, a2: A) -> usize;
}
///L0091
impl<A: UnsignedNum> Translator<A> {}
///L0204
impl<A: UnsignedNum> AddressTranslator<A> for Translator<A> {}
///L0225
variant!(a2p_0000 / p2a_0000, inner = z, outer = z, shift = z, rot = z);
///L0226
variant!(a2p_1000 / p2a_1000, inner = nz, outer = z, shift = z, rot = z);
///L0227
variant!(a2p_0100 / p2a_0100, inner = z, outer = nz, shift = z, rot = z);
///L0228
variant!(a2p_0010 / p2a_0010, inner = z, outer = z, shift = nz, rot = z);
///L0229
variant!(a2p_0001 / p2a_0001, inner = z, outer = z, shift = z, rot = nz);
///L0230
variant!(a2p_1100 / p2a_1100, inner = nz, outer = nz, shift = z, rot = z);
///L0231
variant!(a2p_1010 / p2a_1010, inner = nz, outer = z, shift = nz, rot = z);
///L0232
variant!(a2p_1001 / p2a_1001, inner = nz, outer = z, shift = z, rot = nz);
///L0233
variant!(a2p_0110 / p2a_0110, inner = z, outer = nz, shift = nz, rot = z);
///L0234
variant!(a2p_0101 / p2a_0101, inner = z, outer = nz, shift = z, rot = nz);
///L0235
variant!(a2p_0011 / p2a_0011, inner = z, outer = z, shift = nz, rot = nz);
///L0236
variant!(a2p_1110 / p2a_1110, inner = nz, outer = nz, shift = nz, rot = z);
///L0237
variant!(a2p_1101 / p2a_1101, inner = nz, outer = nz, shift = z, rot = nz);
///L0238
variant!(a2p_1011 / p2a_1011, inner = nz, outer = z, shift = nz, rot = nz);
///L0239
variant!(a2p_0111 / p2a_0111, inner = z, outer = nz, shift = nz, rot = nz);
///L0240
variant!(a2p_1111 / p2a_1111, inner = nz, outer = nz, shift = nz, rot = nz);
```

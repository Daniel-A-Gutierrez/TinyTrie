```rust
//!virtual↔physical address translation, fn-ptr-specialized over the 16
//!(inner/outer/shift/rotation × zero/nonzero) combos so a steady param is
//!straight-line with no per-lookup branch. `v2p` is the hot path; `p2v` runs
//!on remap.
//!
//!invariant: `p2v(p) = ((p + inner_offset) << shift).ror(rotation) + outer_offset`,
//!and `v2p` is the exact inverse — round-trip exact on canonical
//(block-handed-out) vaddrs. vaddrs may wrap; the only hard rule is physical
//!order: phys 0 = min element, phys len−1 = max.
// specialized fn-ptr aliases — the `variant!`/`apply!` macros below generate
// the 16 specialized bodies.
// p2v(p) = ((p + inner_offset) << shift) ror rotation + outer_offset.
// v2p(v) = ((v - outer_offset) rol rotation) >> shift - inner_offset   (exact
// inverse on canonical slots).
// inner_offset lives in physical space (added before the shift); outer_offset
// in virtual space (added after).
// Each op whose param is 0 is a runtime no-op the CPU does NOT elide (see
// bench notes), so specialize picks a pre-baked body that skips zero-param ops
// entirely — straight-line, no per-iter branch, no mispredict risk. Dispatch
// happens once per set_*, not per lookup; the call target is constant for the
// life of the params, so the BTB-predicted indirect call costs ~1 cycle on the v
// chain (see bench).
///L0026
type V2p<P> = fn(P, P, P, u32, u32) -> P; // x, inner, outer, shift, rotation
///L0027
type P2v<P> = fn(P, P, P, u32, u32) -> P;
// apply x.method(arg) only when the param is nonzero (nz); z is a passthrough.
///L0030
macro_rules! apply;
// generate one v2p/p2v pair for a given (inner, outer, shift, rot) nz/z pattern.
// v2p inverts p2v in reverse op order: ror, sub outer, shr, sub inner.
///L0041
macro_rules! variant;
///L0067
///address translator using fn-ptr specialization (see bench notes / v2p_fnptr).
///`set_*` re-points v2p/p2v when the block's params change (grow/spread/graduate).
///for a statically-known strategy, a const-generic block inlines the math and
///beats even this — Translator is for the adaptive tier.
#[derive(Clone)]
pub struct Translator<P> {
    inner_offset: P,
    outer_offset: P,
    shift:        u32,
    rotation:     u32,
    v2p:          V2p<P>,
    p2v:          P2v<P>,
}
///L0078
///virtual <-> physical address translation. P is the in-block pointer type;
///physical slots are usize. v2p is the hot lookup path, p2v runs on remap.
pub trait AddressTranslator<P>: Sized {
    ///virtual address to physical slot
    fn v2p(&self, virt: P) -> usize;
    ///physical slot to virtual address
    fn p2v(&self, phys: usize) -> P;
    ///physical abs distance between two vptrs;
    fn vdist(&self, v1: P, v2: P) -> usize;
}
///L0089
impl<P: UnsignedNum> Translator<P> {}
///L0202
impl<P: UnsignedNum> AddressTranslator<P> for Translator<P> {}
///L0223
variant!(v2p_0000 / p2v_0000, inner = z, outer = z, shift = z, rot = z);
///L0224
variant!(v2p_1000 / p2v_1000, inner = nz, outer = z, shift = z, rot = z);
///L0225
variant!(v2p_0100 / p2v_0100, inner = z, outer = nz, shift = z, rot = z);
///L0226
variant!(v2p_0010 / p2v_0010, inner = z, outer = z, shift = nz, rot = z);
///L0227
variant!(v2p_0001 / p2v_0001, inner = z, outer = z, shift = z, rot = nz);
///L0228
variant!(v2p_1100 / p2v_1100, inner = nz, outer = nz, shift = z, rot = z);
///L0229
variant!(v2p_1010 / p2v_1010, inner = nz, outer = z, shift = nz, rot = z);
///L0230
variant!(v2p_1001 / p2v_1001, inner = nz, outer = z, shift = z, rot = nz);
///L0231
variant!(v2p_0110 / p2v_0110, inner = z, outer = nz, shift = nz, rot = z);
///L0232
variant!(v2p_0101 / p2v_0101, inner = z, outer = nz, shift = z, rot = nz);
///L0233
variant!(v2p_0011 / p2v_0011, inner = z, outer = z, shift = nz, rot = nz);
///L0234
variant!(v2p_1110 / p2v_1110, inner = nz, outer = nz, shift = nz, rot = z);
///L0235
variant!(v2p_1101 / p2v_1101, inner = nz, outer = nz, shift = z, rot = nz);
///L0236
variant!(v2p_1011 / p2v_1011, inner = nz, outer = z, shift = nz, rot = nz);
///L0237
variant!(v2p_0111 / p2v_0111, inner = z, outer = nz, shift = nz, rot = nz);
///L0238
variant!(v2p_1111 / p2v_1111, inner = nz, outer = nz, shift = nz, rot = nz);
```

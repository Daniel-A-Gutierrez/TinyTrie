//!address↔position translation, fn-ptr-specialized over the 16
//!(inner/outer/shift/rotation × zero/nonzero) combos so a steady param is
//!straight-line with no per-lookup branch. `a2p` is the hot path; `p2a` runs
//!on remap.
//!
//!invariant: `p2a(p) = ((p + inner_offset) << shift).ror(rotation) + outer_offset`,
//!and `a2p` is the exact inverse — round-trip exact on canonical
//!(block-handed-out) addrs. addrs may wrap; the only hard rule is position
//!order: pos 0 = min element, pos len−1 = max.

use crate::index::UnsignedNum;
use crate::metadata::Pos;

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
type A2p<A> = fn(A, A, A, u32, u32) -> A; // x, inner, outer, shift, rotation
type P2a<A> = fn(A, A, A, u32, u32) -> A;

// apply x.method(arg) only when the param is nonzero (nz); z is a passthrough.
macro_rules! apply {
    ($x:expr, z, $method:ident, $arg:expr) => {
        $x
    };
    ($x:expr, nz, $method:ident, $arg:expr) => {
        $x.$method($arg)
    };
}

// generate one a2p/p2a pair for a given (inner, outer, shift, rot) nz/z pattern.
// a2p inverts p2a in reverse op order: ror, sub outer, shr, sub inner.
macro_rules! variant {
    ($a2p:ident / $p2a:ident, inner=$i:tt, outer=$o:tt, shift=$s:tt, rot=$r:tt) => {
        #[inline]
        #[allow(unused_variables)]
        fn $a2p<A: UnsignedNum>(x: A, inner: A, outer: A, shift: u32, rotation: u32) -> A {
            let x = apply!(x, $o, wrapping_sub, outer);
            let x = apply!(x, $r, rotate_left, rotation);
            let x = apply!(x, $s, wrapping_shr, shift);
            apply!(x, $i, wrapping_sub, inner)
        }
        #[inline]
        #[allow(unused_variables)]
        fn $p2a<A: UnsignedNum>(x: A, inner: A, outer: A, shift: u32, rotation: u32) -> A {
            let x = apply!(x, $i, wrapping_add, inner);
            let x = apply!(x, $s, wrapping_shl, shift);
            let x = apply!(x, $r, rotate_right, rotation);
            apply!(x, $o, wrapping_add, outer)
        }
    };
}

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

impl<A: UnsignedNum> Translator<A> {
    pub(crate) fn new(inner_offset: A, outer_offset: A, shift: u32, rotation: u32) -> Self {
        Self {
            inner_offset,
            outer_offset,
            shift,
            rotation,
            a2p: a2p_0000::<A>,
            p2a: p2a_0000::<A>,
        }
        .specialize(inner_offset, outer_offset, shift, rotation)
    }
    pub(crate) fn inner_offset(&self) -> A {
        self.inner_offset
    }
    pub(crate) fn outer_offset(&self) -> A {
        self.outer_offset
    }
    pub(crate) fn shift(&self) -> u32 {
        self.shift
    }
    pub(crate) fn rotation(&self) -> u32 {
        self.rotation
    }

    ///per-field setters: re-specialize only when that field's zero/nonzero
    ///status flips. a steady param (e.g. rotation bumping past 1) is a plain
    ///field write — no fn-ptr re-dispatch.
    pub(crate) fn set_inner_offset(&mut self, inner_offset: A) {
        if (self.inner_offset == A::from_usize(0)) != (inner_offset == A::from_usize(0)) {
            self.inner_offset = inner_offset;
            self.specialize_into(inner_offset, self.outer_offset, self.shift, self.rotation);
        } else {
            self.inner_offset = inner_offset;
        }
    }
    pub(crate) fn set_outer_offset(&mut self, outer_offset: A) {
        if (self.outer_offset == A::from_usize(0)) != (outer_offset == A::from_usize(0)) {
            self.outer_offset = outer_offset;
            self.specialize_into(self.inner_offset, outer_offset, self.shift, self.rotation);
        } else {
            self.outer_offset = outer_offset;
        }
    }
    pub(crate) fn set_shift(&mut self, shift: u32) {
        if (self.shift == 0) != (shift == 0) {
            self.shift = shift;
            self.specialize_into(self.inner_offset, self.outer_offset, shift, self.rotation);
        } else {
            self.shift = shift;
        }
    }
    pub(crate) fn set_rotation(&mut self, rotation: u32) {
        if (self.rotation == 0) != (rotation == 0) {
            self.rotation = rotation;
            self.specialize_into(self.inner_offset, self.outer_offset, self.shift, rotation);
        } else {
            self.rotation = rotation;
        }
    }

    fn specialize(self, inner_offset: A, outer_offset: A, shift: u32, rotation: u32) -> Self {
        let mut s = self;
        s.specialize_into(inner_offset, outer_offset, shift, rotation);
        s
    }

    fn specialize_into(&mut self, inner_offset: A, outer_offset: A, shift: u32, rotation: u32) {
        let nz = (
            inner_offset != A::from_usize(0),
            outer_offset != A::from_usize(0),
            shift != 0,
            rotation != 0,
        );
        self.a2p = match nz {
            (false, false, false, false) => a2p_0000::<A>,
            (true, false, false, false) => a2p_1000::<A>,
            (false, true, false, false) => a2p_0100::<A>,
            (false, false, true, false) => a2p_0010::<A>,
            (false, false, false, true) => a2p_0001::<A>,
            (true, true, false, false) => a2p_1100::<A>,
            (true, false, true, false) => a2p_1010::<A>,
            (true, false, false, true) => a2p_1001::<A>,
            (false, true, true, false) => a2p_0110::<A>,
            (false, true, false, true) => a2p_0101::<A>,
            (false, false, true, true) => a2p_0011::<A>,
            (true, true, true, false) => a2p_1110::<A>,
            (true, true, false, true) => a2p_1101::<A>,
            (true, false, true, true) => a2p_1011::<A>,
            (false, true, true, true) => a2p_0111::<A>,
            (true, true, true, true) => a2p_1111::<A>,
        };
        self.p2a = match nz {
            (false, false, false, false) => p2a_0000::<A>,
            (true, false, false, false) => p2a_1000::<A>,
            (false, true, false, false) => p2a_0100::<A>,
            (false, false, true, false) => p2a_0010::<A>,
            (false, false, false, true) => p2a_0001::<A>,
            (true, true, false, false) => p2a_1100::<A>,
            (true, false, true, false) => p2a_1010::<A>,
            (true, false, false, true) => p2a_1001::<A>,
            (false, true, true, false) => p2a_0110::<A>,
            (false, true, false, true) => p2a_0101::<A>,
            (false, false, true, true) => p2a_0011::<A>,
            (true, true, true, false) => p2a_1110::<A>,
            (true, true, false, true) => p2a_1101::<A>,
            (true, false, true, true) => p2a_1011::<A>,
            (false, true, true, true) => p2a_0111::<A>,
            (true, true, true, true) => p2a_1111::<A>,
        };
    }
}

impl<A: UnsignedNum> AddressTranslator<A> for Translator<A> {
    fn a2p(&self, addr: A) -> Pos {
        Pos((self.a2p)(addr, self.inner_offset, self.outer_offset, self.shift, self.rotation)
            .as_usize())
    }

    fn p2a(&self, pos: Pos) -> A {
        (self.p2a)(
            A::from_usize(pos.0),
            self.inner_offset,
            self.outer_offset,
            self.shift,
            self.rotation,
        )
    }

    fn adist(&self, a1: A, a2: A) -> usize {
        self.a2p(a2).0.abs_diff(self.a2p(a1).0)
    }
}

variant!(a2p_0000 / p2a_0000, inner = z, outer = z, shift = z, rot = z);
variant!(a2p_1000 / p2a_1000, inner = nz, outer = z, shift = z, rot = z);
variant!(a2p_0100 / p2a_0100, inner = z, outer = nz, shift = z, rot = z);
variant!(a2p_0010 / p2a_0010, inner = z, outer = z, shift = nz, rot = z);
variant!(a2p_0001 / p2a_0001, inner = z, outer = z, shift = z, rot = nz);
variant!(a2p_1100 / p2a_1100, inner = nz, outer = nz, shift = z, rot = z);
variant!(a2p_1010 / p2a_1010, inner = nz, outer = z, shift = nz, rot = z);
variant!(a2p_1001 / p2a_1001, inner = nz, outer = z, shift = z, rot = nz);
variant!(a2p_0110 / p2a_0110, inner = z, outer = nz, shift = nz, rot = z);
variant!(a2p_0101 / p2a_0101, inner = z, outer = nz, shift = z, rot = nz);
variant!(a2p_0011 / p2a_0011, inner = z, outer = z, shift = nz, rot = nz);
variant!(a2p_1110 / p2a_1110, inner = nz, outer = nz, shift = nz, rot = z);
variant!(a2p_1101 / p2a_1101, inner = nz, outer = nz, shift = z, rot = nz);
variant!(a2p_1011 / p2a_1011, inner = nz, outer = z, shift = nz, rot = nz);
variant!(a2p_0111 / p2a_0111, inner = z, outer = nz, shift = nz, rot = nz);
variant!(a2p_1111 / p2a_1111, inner = nz, outer = nz, shift = nz, rot = nz);

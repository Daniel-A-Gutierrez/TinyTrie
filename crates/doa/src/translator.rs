use crate::index::UnsignedNum;

/// virtual <-> physical address translation. P is the in-block pointer type;
/// physical slots are usize. v2p is the hot lookup path, p2v runs on remap.
pub(crate) trait AddressTranslator<P>: Sized {
    ///virtual address to physical slot
    fn v2p(&self, virt: P) -> usize;

    ///physical slot to virtual address
    fn p2v(&self, phys: usize) -> P;

    ///physical abs distance between two vptrs;
    fn vdist(&self, v1: P, v2: P) -> usize;
}

// p2v(p) = ((p + inner_offset) << shift + outer_offset) rol rotation.
// v2p(v) = ((v ror rotation - outer_offset) >> shift) - inner_offset   (exact
// inverse on canonical slots). inner_offset lives in physical space (added
// before the shift), outer_offset in virtual space (added after); the split
// lets a block pin its root at the beginning/middle/end by tuning the two
// offsets independently across orderings (see doa.md). Each op whose param is
// 0 is a runtime no-op the CPU does NOT elide (see bench notes), so specialize
// picks a pre-baked body that skips zero-param ops entirely — straight-line,
// no per-iter branch, no mispredict risk. Dispatch happens once per set_*,
// not per lookup; the call target is constant for the life of the params, so
// the BTB-predicted indirect call costs ~1 cycle on the v chain (see bench).
type V2p<P> = fn(P, P, P, u32, u32) -> P; // x, inner, outer, shift, rotation
type P2v<P> = fn(P, P, P, u32, u32) -> P;

// apply x.method(arg) only when the param is nonzero (nz); z is a passthrough.
macro_rules! apply {
    ($x:expr, z, $method:ident, $arg:expr) => {
        $x
    };
    ($x:expr, nz, $method:ident, $arg:expr) => {
        $x.$method($arg)
    };
}

// generate one v2p/p2v pair for a given (inner, outer, shift, rot) nz/z pattern.
// v2p inverts p2v in reverse op order: ror, sub outer, shr, sub inner.
macro_rules! variant {
    ($v2p:ident / $p2v:ident, inner=$i:tt, outer=$o:tt, shift=$s:tt, rot=$r:tt) => {
        #[inline]
        #[allow(unused_variables)]
        fn $v2p<P: UnsignedNum>(x: P, inner: P, outer: P, shift: u32, rotation: u32) -> P {
            let x = apply!(x, $r, rotate_right, rotation);
            let x = apply!(x, $o, wrapping_sub, outer);
            let x = apply!(x, $s, wrapping_shr, shift);
            apply!(x, $i, wrapping_sub, inner)
        }
        #[inline]
        #[allow(unused_variables)]
        fn $p2v<P: UnsignedNum>(x: P, inner: P, outer: P, shift: u32, rotation: u32) -> P {
            let x = apply!(x, $i, wrapping_add, inner);
            let x = apply!(x, $s, wrapping_shl, shift);
            let x = apply!(x, $o, wrapping_add, outer);
            apply!(x, $r, rotate_left, rotation)
        }
    };
}

variant!(v2p_0000 / p2v_0000, inner = z, outer = z, shift = z, rot = z);
variant!(v2p_1000 / p2v_1000, inner = nz, outer = z, shift = z, rot = z);
variant!(v2p_0100 / p2v_0100, inner = z, outer = nz, shift = z, rot = z);
variant!(v2p_0010 / p2v_0010, inner = z, outer = z, shift = nz, rot = z);
variant!(v2p_0001 / p2v_0001, inner = z, outer = z, shift = z, rot = nz);
variant!(v2p_1100 / p2v_1100, inner = nz, outer = nz, shift = z, rot = z);
variant!(v2p_1010 / p2v_1010, inner = nz, outer = z, shift = nz, rot = z);
variant!(v2p_1001 / p2v_1001, inner = nz, outer = z, shift = z, rot = nz);
variant!(v2p_0110 / p2v_0110, inner = z, outer = nz, shift = nz, rot = z);
variant!(v2p_0101 / p2v_0101, inner = z, outer = nz, shift = z, rot = nz);
variant!(v2p_0011 / p2v_0011, inner = z, outer = z, shift = nz, rot = nz);
variant!(v2p_1110 / p2v_1110, inner = nz, outer = nz, shift = nz, rot = z);
variant!(v2p_1101 / p2v_1101, inner = nz, outer = nz, shift = z, rot = nz);
variant!(v2p_1011 / p2v_1011, inner = nz, outer = z, shift = nz, rot = nz);
variant!(v2p_0111 / p2v_0111, inner = z, outer = nz, shift = nz, rot = nz);
variant!(v2p_1111 / p2v_1111, inner = nz, outer = nz, shift = nz, rot = nz);

///address translator using fn-ptr specialization (see bench notes / v2p_fnptr).
///adaptive tier shape: re-point v2p/p2v in set_params when the block's params
///change (grow/spread/graduate). for a statically-known strategy, a const-generic
///block inlines the math and beats even this — Translator is for the adaptive tier.
pub(crate) struct Translator<P: UnsignedNum> {
    inner_offset: P,
    outer_offset: P,
    shift:        u32,
    rotation:     u32,
    v2p:          V2p<P>,
    p2v:          P2v<P>,
}

impl<P: UnsignedNum> Translator<P> {
    pub(crate) fn new(inner_offset: P, outer_offset: P, shift: u32, rotation: u32) -> Self {
        Self {
            inner_offset,
            outer_offset,
            shift,
            rotation,
            v2p: v2p_0000::<P>,
            p2v: p2v_0000::<P>,
        }
        .specialize(inner_offset, outer_offset, shift, rotation)
    }
    pub(crate) fn inner_offset(&self) -> P {
        self.inner_offset
    }
    pub(crate) fn outer_offset(&self) -> P {
        self.outer_offset
    }
    pub(crate) fn shift(&self) -> u32 {
        self.shift
    }
    pub(crate) fn rotation(&self) -> u32 {
        self.rotation
    }

    ///re-point the specialized bodies after a param change (one indirect call
    ///per lookup thereafter, no per-iter branch).
    pub(crate) fn set_params(
        &mut self,
        inner_offset: P,
        outer_offset: P,
        shift: u32,
        rotation: u32,
    ) {
        self.inner_offset = inner_offset;
        self.outer_offset = outer_offset;
        self.shift = shift;
        self.rotation = rotation;
        self.specialize_into(inner_offset, outer_offset, shift, rotation);
    }

    ///per-field setters: re-specialize only when that field's zero/nonzero
    ///status flips. a steady param (e.g. rotation bumping past 1) is a plain
    ///field write — no fn-ptr re-dispatch.
    pub(crate) fn set_inner_offset(&mut self, inner_offset: P) {
        if (self.inner_offset == P::from_usize(0)) != (inner_offset == P::from_usize(0)) {
            self.inner_offset = inner_offset;
            self.specialize_into(inner_offset, self.outer_offset, self.shift, self.rotation);
        } else {
            self.inner_offset = inner_offset;
        }
    }
    pub(crate) fn set_outer_offset(&mut self, outer_offset: P) {
        if (self.outer_offset == P::from_usize(0)) != (outer_offset == P::from_usize(0)) {
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

    fn specialize(self, inner_offset: P, outer_offset: P, shift: u32, rotation: u32) -> Self {
        let mut s = self;
        s.specialize_into(inner_offset, outer_offset, shift, rotation);
        s
    }

    fn specialize_into(&mut self, inner_offset: P, outer_offset: P, shift: u32, rotation: u32) {
        let nz = (
            inner_offset != P::from_usize(0),
            outer_offset != P::from_usize(0),
            shift != 0,
            rotation != 0,
        );
        self.v2p = match nz {
            (false, false, false, false) => v2p_0000::<P>,
            (true, false, false, false) => v2p_1000::<P>,
            (false, true, false, false) => v2p_0100::<P>,
            (false, false, true, false) => v2p_0010::<P>,
            (false, false, false, true) => v2p_0001::<P>,
            (true, true, false, false) => v2p_1100::<P>,
            (true, false, true, false) => v2p_1010::<P>,
            (true, false, false, true) => v2p_1001::<P>,
            (false, true, true, false) => v2p_0110::<P>,
            (false, true, false, true) => v2p_0101::<P>,
            (false, false, true, true) => v2p_0011::<P>,
            (true, true, true, false) => v2p_1110::<P>,
            (true, true, false, true) => v2p_1101::<P>,
            (true, false, true, true) => v2p_1011::<P>,
            (false, true, true, true) => v2p_0111::<P>,
            (true, true, true, true) => v2p_1111::<P>,
        };
        self.p2v = match nz {
            (false, false, false, false) => p2v_0000::<P>,
            (true, false, false, false) => p2v_1000::<P>,
            (false, true, false, false) => p2v_0100::<P>,
            (false, false, true, false) => p2v_0010::<P>,
            (false, false, false, true) => p2v_0001::<P>,
            (true, true, false, false) => p2v_1100::<P>,
            (true, false, true, false) => p2v_1010::<P>,
            (true, false, false, true) => p2v_1001::<P>,
            (false, true, true, false) => p2v_0110::<P>,
            (false, true, false, true) => p2v_0101::<P>,
            (false, false, true, true) => p2v_0011::<P>,
            (true, true, true, false) => p2v_1110::<P>,
            (true, true, false, true) => p2v_1101::<P>,
            (true, false, true, true) => p2v_1011::<P>,
            (false, true, true, true) => p2v_0111::<P>,
            (true, true, true, true) => p2v_1111::<P>,
        };
    }
}

impl<P: UnsignedNum> AddressTranslator<P> for Translator<P> {
    fn v2p(&self, virt: P) -> usize {
        (self.v2p)(virt, self.inner_offset, self.outer_offset, self.shift, self.rotation)
            .as_usize()
    }

    fn p2v(&self, phys: usize) -> P {
        (self.p2v)(
            P::from_usize(phys),
            self.inner_offset,
            self.outer_offset,
            self.shift,
            self.rotation,
        )
    }

    fn vdist(&self, v1: P, v2: P) -> usize {
        self.v2p(v2).abs_diff(self.v2p(v1))
    }
}

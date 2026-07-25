use crate::index::UnsignedNum;
/// virtual <-> physical address translation. P is the in-block pointer type;
/// physical slots are usize. v2p is the hot lookup path, p2v runs on remap.
pub(crate) trait AddressTranslator<P>: Sized {
    ///virtual address to physical slot
    fn v2p(&self, virt: P) -> usize;
    ///physical slot to virtual address
    fn p2v(&self, phys: usize) -> P;
    ///physical distance between two vptrs; caller guarantees v1 < v2 in physical space.
    fn vdist(&self, v1: P, v2: P) -> usize;
}
// v2p(v) = ((v + offset) << shift) ror rotation. p2v is the exact inverse:
// (p rol rotation) >> shift - offset. Each op whose param is 0 is a runtime
// no-op the CPU does NOT elide (see bench notes), so set_params picks a
// pre-baked body that skips zero-param ops entirely — straight-line, no
// per-iter branch, no mispredict risk. Dispatch happens once per set_params,
// not per lookup; the call target is constant for the life of the params, so
// the BTB-predicted indirect call costs ~1 cycle on the v chain (see bench).
type V2p<P> = fn(P, P, u32, u32) -> P;
type P2v<P> = fn(P, P, u32, u32) -> P;
fn v2p_id<P: UnsignedNum>(x: P, _o: P, _s: u32, _r: u32) -> P {
    x
}
fn v2p_o<P: UnsignedNum>(x: P, o: P, _s: u32, _r: u32) -> P {
    x.wrapping_add(o)
}
fn v2p_s<P: UnsignedNum>(x: P, _o: P, s: u32, _r: u32) -> P {
    x.wrapping_shl(s)
}
fn v2p_r<P: UnsignedNum>(x: P, _o: P, _s: u32, r: u32) -> P {
    x.rotate_right(r)
}
fn v2p_os<P: UnsignedNum>(x: P, o: P, s: u32, _r: u32) -> P {
    x.wrapping_add(o).wrapping_shl(s)
}
fn v2p_or<P: UnsignedNum>(x: P, o: P, _s: u32, r: u32) -> P {
    x.wrapping_add(o).rotate_right(r)
}
fn v2p_sr<P: UnsignedNum>(x: P, _o: P, s: u32, r: u32) -> P {
    x.wrapping_shl(s).rotate_right(r)
}
fn v2p_osr<P: UnsignedNum>(x: P, o: P, s: u32, r: u32) -> P {
    x.wrapping_add(o).wrapping_shl(s).rotate_right(r)
}
fn p2v_id<P: UnsignedNum>(x: P, _o: P, _s: u32, _r: u32) -> P {
    x
}
fn p2v_o<P: UnsignedNum>(x: P, o: P, _s: u32, _r: u32) -> P {
    x.wrapping_sub(o)
}
fn p2v_s<P: UnsignedNum>(x: P, _o: P, s: u32, _r: u32) -> P {
    x.wrapping_shr(s)
}
fn p2v_r<P: UnsignedNum>(x: P, _o: P, _s: u32, r: u32) -> P {
    x.rotate_left(r)
}
fn p2v_os<P: UnsignedNum>(x: P, o: P, s: u32, _r: u32) -> P {
    x.wrapping_shr(s).wrapping_sub(o)
}
fn p2v_or<P: UnsignedNum>(x: P, o: P, _s: u32, r: u32) -> P {
    x.rotate_left(r).wrapping_sub(o)
}
fn p2v_sr<P: UnsignedNum>(x: P, _o: P, s: u32, r: u32) -> P {
    x.rotate_left(r).wrapping_shr(s)
}
fn p2v_osr<P: UnsignedNum>(x: P, o: P, s: u32, r: u32) -> P {
    x.rotate_left(r).wrapping_shr(s).wrapping_sub(o)
}
///address translator using fn-ptr specialization (see bench notes / v2p_fnptr).
///adaptive tier shape: re-point v2p/p2v in set_params when the block's params
///change (grow/spread/graduate). for a statically-known strategy, a const-generic
///block inlines the math and beats even this — Translator is for the adaptive tier.
pub(crate) struct Translator<P: UnsignedNum> {
    offset:   P,
    shift:    u32,
    rotation: u32,
    v2p:      V2p<P>,
    p2v:      P2v<P>,
}
impl<P: UnsignedNum> Translator<P> {
    pub(crate) fn new(offset: P, shift: u32, rotation: u32) -> Self {
        Self { offset, shift, rotation, v2p: v2p_id::<P>, p2v: p2v_id::<P> }
            .specialize(offset, shift, rotation)
    }
    ///re-point the specialized bodies after a param change (one indirect call
    ///per lookup thereafter, no per-iter branch).
    pub(crate) fn set_params(&mut self, offset: P, shift: u32, rotation: u32) {
        self.offset = offset;
        self.shift = shift;
        self.rotation = rotation;
        self.specialize_into(offset, shift, rotation);
    }
    fn specialize(self, offset: P, shift: u32, rotation: u32) -> Self {
        let mut s = self;
        s.specialize_into(offset, shift, rotation);
        s
    }
    fn specialize_into(&mut self, offset: P, shift: u32, rotation: u32) {
        let nz = (offset != P::from_usize(0), shift != 0, rotation != 0);
        self.v2p = match nz {
            (false, false, false) => v2p_id::<P>,
            (true, false, false) => v2p_o::<P>,
            (false, true, false) => v2p_s::<P>,
            (false, false, true) => v2p_r::<P>,
            (true, true, false) => v2p_os::<P>,
            (true, false, true) => v2p_or::<P>,
            (false, true, true) => v2p_sr::<P>,
            (true, true, true) => v2p_osr::<P>,
        };
        self.p2v = match nz {
            (false, false, false) => p2v_id::<P>,
            (true, false, false) => p2v_o::<P>,
            (false, true, false) => p2v_s::<P>,
            (false, false, true) => p2v_r::<P>,
            (true, true, false) => p2v_os::<P>,
            (true, false, true) => p2v_or::<P>,
            (false, true, true) => p2v_sr::<P>,
            (true, true, true) => p2v_osr::<P>,
        };
    }
}
impl<P: UnsignedNum> AddressTranslator<P> for Translator<P> {
    fn v2p(&self, virt: P) -> usize {
        (self.v2p)(virt, self.offset, self.shift, self.rotation).as_usize()
    }
    fn p2v(&self, phys: usize) -> P {
        (self.p2v)(P::from_usize(phys), self.offset, self.shift, self.rotation)
    }
    fn vdist(&self, v1: P, v2: P) -> usize {
        self.v2p(v2) - self.v2p(v1)
    }
}

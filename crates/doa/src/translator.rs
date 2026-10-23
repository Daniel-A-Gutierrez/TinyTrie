//!the set/map tier's 2-knob translator — one inline path per op, a branch
//!on `reversed` picking the operand order, rotation+offset applied
//!unconditionally: forward `p2a(p) = (p − offset) ror rotation`, reversed
//!`p2a(p) = (offset − p) ror rotation`. deliberately fn-ptr-free: a
//!memmapped block would serialize the translator verbatim, and a stored
//!code address is a hijack primitive — the branch shape measured ~4
//!cycles/call faster anyway (benches/translator.rs).
//!invariant: a2p is p2a's exact inverse (round-trip exact on canonical
//!slots); names survive the rotation turn (`pos → 2·pos` under BOTH
//!directions — offset doubling cancels the negation) and the reversal
//!(`p2a_rev_new(p) = p2a_old(cap−1−p)`, names follow the mirrored slots).

use crate::index::*;

///the set tier's 2-knob translator — rotation turn = growth (doubling
///rule unchanged either direction), reversal re-parameterizes so names
///follow the mirrored slots
#[derive(Clone, Copy)]
pub struct Translator<A> {
    rotation: u32,
    offset:   A,
    reversed: bool,
}

impl<A: UnsignedNum> Translator<A> {
    pub(crate) fn new() -> Self {
        Translator { rotation: 0, offset: A::ZERO, reversed: false }
    }

    pub(crate) fn reversed(&self) -> bool {
        self.reversed
    }

    pub(crate) fn rotation(&self) -> u32 {
        self.rotation
    }

    pub(crate) fn offset(&self) -> A {
        self.offset
    }

    ///arbitrary params, forward — e.g. the offset≠0 states grow_rotate
    ///refuses today; the maps must still hold for them
    #[cfg(test)]
    pub(crate) fn with_params(rotation: u32, offset: A) -> Self {
        Translator { rotation, offset, ..Translator::new() }
    }

    ///the cleave shift: positions slide down by `at` with names intact —
    ///`p2a_new(p) = p2a_old(p + at)` under BOTH directions (offset − at,
    ///wrapping). the cleaved half's elements keep their names; only
    ///their block changes
    pub(crate) fn shifted(&self, at: usize) -> Self {
        Translator {
            offset: self.offset.wrapping_sub(A::from_usize(at)),
            ..*self
        }
    }

    ///the rotation turn: rotation + 1, offset doubled — names preserved by
    ///construction under both directions
    pub(crate) fn rotate(&self) -> Self {
        //forward: the old rotation cancels leaving `pos → 2·pos`;
        //reversed: the doubling cancels the negation — same map
        Translator {
            rotation: self.rotation + 1,
            offset:   self.offset.wrapping_add(self.offset),
            ..*self
        }
    }

    ///the reversal: `p2a_new(p) = p2a_old(cap−1−p)` — names follow the
    ///mirrored slots, holders see nothing
    pub(crate) fn reverse(&self, cap: usize) -> Self {
        //offset ≠ 0 afterward (cap > 1), so rotation turns refuse until
        //offset turns land
        Translator {
            offset:   A::from_usize(cap - 1).wrapping_sub(self.offset),
            reversed: !self.reversed,
            ..*self
        }
    }

    #[inline(always)]
    pub(crate) fn a2p(&self, a: A) -> Pos {
        let t = a.rotate_left(self.rotation);
        Pos(if self.reversed {
            self.offset.wrapping_sub(t)
        } else {
            t.wrapping_add(self.offset)
        }.as_usize())
    }

    #[inline(always)]
    pub(crate) fn p2a(&self, p: Pos) -> A {
        let x = A::from_usize(p.0);
        let t = if self.reversed {
            self.offset.wrapping_sub(x)
        } else {
            x.wrapping_sub(self.offset)
        };
        t.rotate_right(self.rotation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    ///round-trip exactness + the reversal name-mirror + the reversed turn
    ///map (upper-half names land odd, one down: `q = 2p − [(offset−p) mod
    ///2^w ≥ 2^(w−1)]`), every reachable translator
    #[test]
    fn names_survive_reversal_and_turns() {
        const SPACE: usize = 256;
        const MID: usize = 128;
        for r in 0..u8::BIT_WIDTH as u32 {
            for cap in [4usize, 9, 64, 100] {
                //offset < cap in every reachable state (name 0's slot);
                //offset turns will relax this
                for o in (0..cap as u8).step_by((cap / 25).max(1)).chain([0u8, 1]) {
                    let t = Translator::<u8>::with_params(r, o);
                    let rev = t.reverse(cap);
                    let off = rev.offset();
                    let back = rev.reverse(cap);
                    for p in 0..cap {
                        //reversal: the name at the mirrored slot is the old name
                        assert_eq!(rev.p2a(Pos(cap - 1 - p)), t.p2a(Pos(p)), "r={r} o={o} p={p}");
                        //round-trip both directions
                        assert_eq!(rev.a2p(rev.p2a(Pos(p))).0, p, "rev r={r} o={o} p={p}");
                        assert_eq!(t.a2p(t.p2a(Pos(p))).0, p, "fwd r={r} o={o} p={p}");
                        //double reversal = identity
                        assert_eq!(back.p2a(Pos(p)), t.p2a(Pos(p)), "back r={r} o={o} p={p}");
                    }
                    //the reversed turn: names hold, q lands odd over the
                    //band, monotone, q ≥ p (the descending pass's safety)
                    let turned = rev.rotate();
                    let mut prev = 0usize;
                    for p in 0..cap {
                        let a = rev.p2a(Pos(p));
                        let q = turned.a2p(a).0;
                        assert_eq!(turned.p2a(Pos(q)), a, "name r={r} o={o} p={p}");
                        let band = usize::from((off as usize + SPACE - p) % SPACE >= MID);
                        assert_eq!(q, (2 * p + SPACE - band) % SPACE, "map r={r} o={o} p={p}");
                        assert!(q >= prev, "monotone r={r} o={o} p={p}");
                        assert!(q >= p, "descend-safe r={r} o={o} p={p}");
                        prev = q;
                    }
                }
            }
        }
    }
}
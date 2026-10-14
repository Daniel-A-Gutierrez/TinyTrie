//!sortedblock's 2-knob translator — fn-ptr-specialized per direction so
//!translation carries no negate/branch: forward `p2a(p) = (p − offset) ror
//!rotation`, reversed `p2a(p) = (offset − p) ror rotation`. the indirect
//!call is BTB-predicted (~1 cycle, see treeblock translator's bench
//!notes), re-dispatched only on rotate/reverse — `reversed` selects the
//!fns, never touches the math.
//!invariant: a2p is p2a's exact inverse (round-trip exact on canonical
//!slots); names survive the rotation turn (`pos → 2·pos` under BOTH
//!directions — offset doubling cancels the negation) and the reversal
//!(`p2a_rev_new(p) = p2a_old(cap−1−p)`, names follow the mirrored slots).

use crate::index::*;

type A2p<A> = fn(A, u32, A) -> A;
type P2a<A> = fn(Pos, u32, A) -> A;

fn a2p_fwd<A: UnsignedNum>(a: A, rotation: u32, offset: A) -> A {
    a.rotate_left(rotation).wrapping_add(offset)
}

fn a2p_rev<A: UnsignedNum>(a: A, rotation: u32, offset: A) -> A {
    offset.wrapping_sub(a.rotate_left(rotation))
}

fn p2a_fwd<A: UnsignedNum>(p: Pos, rotation: u32, offset: A) -> A {
    A::from_usize(p.0).wrapping_sub(offset).rotate_right(rotation)
}

fn p2a_rev<A: UnsignedNum>(p: Pos, rotation: u32, offset: A) -> A {
    offset.wrapping_sub(A::from_usize(p.0)).rotate_right(rotation)
}

///`a2p(a) = (a rol rotation) + offset` forward, `a2p(a) = offset − (a rol
///rotation)` reversed — the direction knob of the set tier. rotation turn =
///growth (doubling rule unchanged either direction); reversal mirrors the
///slot array and re-parameterizes so names follow the slots.
#[derive(Clone, Copy)]
pub struct Translator<A> {
    rotation: u32,
    offset:   A,
    reversed: bool,
    a2p:      A2p<A>,
    p2a:      P2a<A>,
}

impl<A: UnsignedNum> Translator<A> {
    pub(crate) fn new() -> Self {
        Translator {
            rotation: 0,
            offset:   A::ZERO,
            reversed: false,
            a2p:      a2p_fwd,
            p2a:      p2a_fwd,
        }
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

    ///the rotation turn: rotation + 1, offset doubled — names preserved by
    ///construction under both directions (forward: the old rotation cancels
    ///leaving `pos → 2·pos`; reversed: the doubling cancels the negation,
    ///same map)
    pub(crate) fn rotate(&self) -> Self {
        Translator {
            rotation: self.rotation + 1,
            offset:   self.offset.wrapping_add(self.offset),
            ..*self
        }
    }

    ///the reversal: `p2a_new(p) = p2a_old(cap−1−p)` — offset mirrors to
    ///`cap−1−offset`, direction flips. names follow the mirrored slots;
    ///holders see nothing. offset ≠ 0 afterward (cap > 1), so rotation
    ///turns refuse until offset turns land
    pub(crate) fn reverse(&self, cap: usize) -> Self {
        Translator {
            offset:   A::from_usize(cap - 1).wrapping_sub(self.offset),
            reversed: !self.reversed,
            a2p:      if self.reversed { a2p_fwd } else { a2p_rev },
            p2a:      if self.reversed { p2a_fwd } else { p2a_rev },
            ..*self
        }
    }

    #[inline]
    pub(crate) fn a2p(&self, a: A) -> Pos {
        Pos((self.a2p)(a, self.rotation, self.offset).as_usize())
    }

    #[inline]
    pub(crate) fn p2a(&self, p: Pos) -> A {
        (self.p2a)(p, self.rotation, self.offset)
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
```rust
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
///L0014
type A2p<A> = fn(A, u32, A) -> A;
///L0015
type P2a<A> = fn(Pos, u32, A) -> A;
///L0017
fn a2p_fwd<A: UnsignedNum>(a: A, rotation: u32, offset: A) -> A;
///L0021
fn a2p_rev<A: UnsignedNum>(a: A, rotation: u32, offset: A) -> A;
///L0025
fn p2a_fwd<A: UnsignedNum>(p: Pos, rotation: u32, offset: A) -> A;
///L0029
fn p2a_rev<A: UnsignedNum>(p: Pos, rotation: u32, offset: A) -> A;
///L0037
///the set tier's 2-knob translator — rotation turn = growth (doubling
///rule unchanged either direction), reversal re-parameterizes so names
///follow the mirrored slots
#[derive(Clone, Copy)]
pub struct Translator<A> {
    rotation: u32,
    offset:   A,
    reversed: bool,
    a2p:      A2p<A>,
    p2a:      P2a<A>,
}
///L0045
impl<A: UnsignedNum> Translator<A> {}
///L0113
#[cfg(test)]
mod tests {
    use super::*;
    ///round-trip exactness + the reversal name-mirror + the reversed turn
    ///map (upper-half names land odd, one down: `q = 2p − [(offset−p) mod
    ///2^w ≥ 2^(w−1)]`), every reachable translator
    #[test]
    fn names_survive_reversal_and_turns();
}
```

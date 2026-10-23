```rust
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
///L0019
///the set tier's 2-knob translator — rotation turn = growth (doubling
///rule unchanged either direction), reversal re-parameterizes so names
///follow the mirrored slots
#[derive(Clone, Copy)]
pub struct Translator<A> {
    rotation: u32,
    offset:   A,
    reversed: bool,
}
///L0025
impl<A: UnsignedNum> Translator<A> {}
///L0107
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

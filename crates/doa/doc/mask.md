```rust
//!dense occupancy bitmap — bit p ⇔ slot p live. word-at-a-time scans
//!(`next_set`/`next_clear` skip full/empty words, no per-bit loop); bits past
//! `bits` read as clear, so `next_clear` never returns ≥ `bits`.
///L0007
///invariant: `words.len() == (bits + 63) / 64`; no bit ≥ `bits` is ever set.
#[derive(Clone, Debug, Default)]
pub struct Mask {
    words: Vec<u64>,
    bits:  usize,
}
///L0012
impl Mask {}
///L0278
///bits of x moved to the even positions of a u64 (p → 2p)
fn spread(x: u32) -> u64;
///L0289
#[cfg(test)]
mod tests {
    use super::*;
    fn mask_ones(bits: usize, set: &[usize]) -> Mask;
    #[test]
    fn scans_match_naive();
    #[test]
    fn edges();
    #[test]
    fn shrink_clears_tail();
    #[test]
    fn iter_set_ascending();
    #[test]
    fn reverse_mirrors_bits();
    #[test]
    fn rotate_mask_plain_doubles();
    ///bit p → 2p + [(p − offset) mod name space ≥ midpoint] at every
    ///offset — prefix, suffix, whole-range and empty bands, word-edge and
    ///mid-word junctions
    #[test]
    fn rotate_mask_matches_naive();
    ///reversed turn: bit p → 2p − [name ∈ upper half] — the upper-half
    ///names form the suffix past the wrap point (a2p(0) = offset), landing
    ///one slot DOWN. the down-ripple crosses word boundaries (a word's
    ///bit-0 element falls to the word below's bit 63), so word-edge cases
    ///get their own sweep. offset < bits ≤ midpoint in every reachable
    ///reversed state
    #[test]
    fn rotate_mask_reversed_matches_naive();
}
```

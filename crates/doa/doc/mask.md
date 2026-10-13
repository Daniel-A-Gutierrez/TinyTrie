```rust
//!dense occupancy bitmap — bit p ⇔ slot p live. word-at-a-time scans
//!(`next_set`/`next_clear` skip full/empty words, no per-bit loop); bits past
//! `bits` read as clear, so `next_clear` never returns ≥ `bits`.
///L0007
///invariant: `words.len() == (bits + 63) / 64`; no bit ≥ `bits` is ever set.
#[derive(Clone, Debug, Default)]
pub struct Mask {
    words: Vec<u64>,
    bits: usize,
}
///L0012
impl Mask {}
///L0167
///bits of x moved to the even positions of a u64 (p → 2p)
fn spread(x: u32) -> u64;
///L0178
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
    fn spread_double_spaces_bits();
}
```

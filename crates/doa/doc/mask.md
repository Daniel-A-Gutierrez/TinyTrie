```rust
//!dense occupancy bitmap — bit p ⇔ slot p live. word-at-a-time scans
//!(`next_set`/`next_clear` skip full/empty words, no per-bit loop);
//!`iter_set`/`iter_set_rev` test positions in place against the cached
//!current word. bits past `bits` read as clear, so `next_clear` never
//!returns ≥ `bits`.
///L0009
///invariant: `words.len() == (bits + 63) / 64`; no bit ≥ `bits` is ever set.
#[derive(Clone, Debug, Default)]
pub struct Mask {
    words: Vec<u64>,
    bits:  usize,
}
///L0014
impl Mask {}
///L0318
///`iter_set`'s engine: the current word cached, positions tested in
///place — the dense common case is a bit test + increment (no tz, no
///position rebuild); holes advance one bit, whole-empty word tails skip
///at once. a rotating-bit-mask engine measured slightly WORSE: the shift
///saved on the test returns on the yield as `pos + tzcnt` (the position
///no longer is the state) — see bench_notes
pub struct SetBits<'a> {
    words: std::slice::Iter<'a, u64>,
    ///cached current word; 0 past the end
    word: u64,
    ///next position to test
    pos:  usize,
}
///L0326
impl Iterator for SetBits<'_> {}
///L0365
///`iter_set_rev`'s engine — the descending mirror: positions tested down
///through the cached word, holes advancing one bit, whole-empty word heads
///skipping at once. `pos` wraps below 0 into a sentinel once the mask is
///exhausted
pub struct SetBitsRev<'a> {
    words: std::iter::Rev<std::slice::Iter<'a, u64>>,
    word:  u64,
    ///next position to test
    pos:   usize,
}
///L0372
impl Iterator for SetBitsRev<'_> {}
///L0409
///bits of x moved to the even positions of a u64 (p → 2p)
fn spread(x: u32) -> u64;
///L0420
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
    fn iter_set_rev_descending();
    ///poll twice past the end — exhaustion must clear the cached word, or
    ///a re-poll tests the stale word at boundary positions and yields
    ///out-of-range positions (→ panic through BlockSet::iter's key read)
    #[test]
    fn iterators_none_stable();
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

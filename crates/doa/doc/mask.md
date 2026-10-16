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
///L0362
///`iter_set`'s engine — ascending set bits. None-stable (exhaustion
///zeroes the cached word, subtle_bugs §21)
pub struct SetBits<'a> {
    words: std::slice::Iter<'a, u64>,
    ///cached current word; 0 past the end
    word: u64,
    ///next position to test
    pos:  usize,
}
///L0370
impl Iterator for SetBits<'_> {}
///L0408
///`iter_set_rev`'s engine — the descending mirror. None-stable like
///`SetBits`
pub struct SetBitsRev<'a> {
    words: std::iter::Rev<std::slice::Iter<'a, u64>>,
    word:  u64,
    ///next position to test; wraps below 0 as the exhaustion sentinel
    pos:   usize,
}
///L0415
impl Iterator for SetBitsRev<'_> {}
///L0452
///bits of x moved to the even positions of a u64 (p → 2p)
fn spread(x: u32) -> u64;
///L0463
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
    ///the cursor steppers vs naive at every start position — single
    ///step each way, plus a full walk maintaining the cache across word
    ///crossings and the partial tail word (cache invariant checked per
    ///step). 70 bits: a full word + partial tail, sized for miri
    #[test]
    fn steps_match_naive();
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

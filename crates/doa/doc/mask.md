```rust
//!dense occupancy bitmap — bit p ⇔ slot p live. word-at-a-time scans
//!(`next_set`/`next_clear` skip full/empty words, no per-bit loop);
//!`iter_set`/`iter_set_rev` test positions in place against the cached
//!current word; `MaskCursor` is the bidirectional cached-word walk the
//!set/map cursors wrap. bits past `bits` read as clear, so `next_clear`
//!never returns ≥ `bits`.
///L0010
///invariant: `words.len() == (bits + 63) / 64`; no bit ≥ `bits` is ever set.
#[derive(Clone, Debug, Default)]
pub struct Mask {
    words: Vec<u64>,
    bits:  usize,
}
///L0015
impl Mask {}
///L0405
///bidirectional walk over set bits on a cached word — the walk machine
///the set/map cursors wrap. the tiers own key-order semantics (where to
///land); this owns the stepping, the word cache, and the walk-end states.
///`dir`: positions ascend when true — baked by the tier at construction;
///direction only changes by mutation, which invalidates the walk anyway.
///`slot`: the current slot, or the forward walk's exhaustion ghost —
///`advance` past the last slot ghosts. backward exhaustion needs no
///ghost: `retreat` at the walk's start leaves the slot live
pub(crate) struct MaskCursor<'a> {
    mask: &'a Mask,
    dir:  bool,
    ///the next-to-yield slot, or the forward ghost
    slot: Option<usize>,
    ///Some(p) ⇒ p's word; the ghost leaves it stale (at() reloads)
    word: u64,
}
///L0414
impl<'a> MaskCursor<'a> {}
///L0492
///`iter_set`'s engine — ascending set bits. None-stable (exhaustion
///zeroes the cached word, subtle_bugs §21)
pub struct SetBits<'a> {
    words: std::slice::Iter<'a, u64>,
    ///cached current word; 0 past the end
    word: u64,
    ///next position to test
    pos:  usize,
}
///L0500
impl Iterator for SetBits<'_> {}
///L0538
///`iter_set_rev`'s engine — the descending mirror. None-stable like
///`SetBits`
pub struct SetBitsRev<'a> {
    words: std::iter::Rev<std::slice::Iter<'a, u64>>,
    word:  u64,
    ///next position to test; wraps below 0 as the exhaustion sentinel
    pos:   usize,
}
///L0545
impl Iterator for SetBitsRev<'_> {}
///L0582
///bits of x moved to the even positions of a u64 (p → 2p)
fn spread(x: u32) -> u64;
///L0593
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
    ///copy_region/count_below vs naive at every word-edge combination —
    ///misaligned starts, partial tails, empty and whole-range regions
    #[test]
    fn region_copy_matches_naive();
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

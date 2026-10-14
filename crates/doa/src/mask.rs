//!dense occupancy bitmap — bit p ⇔ slot p live. word-at-a-time scans
//!(`next_set`/`next_clear` skip full/empty words, no per-bit loop);
//!`iter_set`/`iter_set_rev` test positions in place against the cached
//!current word. bits past `bits` read as clear, so `next_clear` never
//!returns ≥ `bits`.

///invariant: `words.len() == (bits + 63) / 64`; no bit ≥ `bits` is ever set.
#[derive(Clone, Debug, Default)]
pub struct Mask {
    words: Vec<u64>,
    bits:  usize,
}

impl Mask {
    pub fn new() -> Self {
        Mask { words: Vec::new(), bits: 0 }
    }

    ///zeroed, `bits` long
    pub fn with_bits(bits: usize) -> Self {
        Mask { words: vec![0; (bits + 63) / 64], bits }
    }

    ///tracked bit count
    pub fn bits(&self) -> usize {
        self.bits
    }

    ///grow/shrink the tracked range; set bits are preserved on grow, and a
    ///shrinking range clears them (the no-bit-past-`bits` invariant holds both
    ///ways)
    pub fn resize(&mut self, bits: usize) {
        self.words.resize((bits + 63) / 64, 0);
        self.bits = bits;
        let tail = bits % 64;
        if tail != 0 {
            if let Some(last) = self.words.last_mut() {
                *last &= (1 << tail) - 1;
            }
        }
    }

    ///panics if p ≥ bits
    pub fn get(&self, p: usize) -> bool {
        assert!(p < self.bits, "mask index {p} ≥ {}", self.bits);
        self.words[p >> 6] & 1 << (p & 63) != 0
    }

    ///panics if p ≥ bits
    pub fn set(&mut self, p: usize, live: bool) {
        assert!(p < self.bits, "mask index {p} ≥ {}", self.bits);
        if live {
            self.words[p >> 6] |= 1 << (p & 63);
        } else {
            self.words[p >> 6] &= !(1 << (p & 63));
        }
    }

    ///first set bit ≥ from, or None
    #[inline]
    pub fn next_set(&self, from: usize) -> Option<usize> {
        //dense fast path: the bit at `from` is usually set — `from` is the
        //answer, no scan and no position rebuild
        if from >> 6 < self.words.len() && self.words[from >> 6] & 1 << (from & 63) != 0 {
            return Some(from);
        }
        let mut w = from >> 6;
        let mut off = from & 63;
        while w < self.words.len() {
            let word = self.words[w] >> off;
            if word != 0 {
                return Some((w << 6) + off + word.trailing_zeros() as usize);
            }
            w += 1;
            off = 0;
        }
        None
    }

    ///highest set bit < from, or None
    #[inline]
    pub fn prev_set(&self, from: usize) -> Option<usize> {
        let from = from.min(self.bits);
        if from == 0 {
            return None;
        }
        //dense fast path: the bit just below is usually set
        let p = from - 1;
        if self.words[p >> 6] & 1 << (p & 63) != 0 {
            return Some(p);
        }
        let mut w = p >> 6;
        let mut keep = (p & 63) + 1;
        loop {
            let word = self.words[w] & if keep == 64 { u64::MAX } else { (1 << keep) - 1 };
            if word != 0 {
                return Some((w << 6) + 63 - word.leading_zeros() as usize);
            }
            if w == 0 {
                return None;
            }
            w -= 1;
            keep = 64;
        }
    }

    ///highest set bit, or None
    pub fn last_set(&self) -> Option<usize> {
        let mut w = self.words.len();
        while w > 0 {
            w -= 1;
            let word = self.words[w];
            if word != 0 {
                return Some((w << 6) + 63 - word.leading_zeros() as usize);
            }
        }
        None
    }

    ///first clear bit ≥ from and < bits, or None
    #[inline]
    pub fn next_clear(&self, from: usize) -> Option<usize> {
        if from >= self.bits {
            return None;
        }
        //fast path: `from` itself clear — it's the answer
        if self.words[from >> 6] & 1 << (from & 63) == 0 {
            return Some(from);
        }
        let mut w = from >> 6;
        let mut off = from & 63;
        while w < self.words.len() {
            let end = (self.bits - (w << 6)).min(64);
            let mut word = !self.words[w] >> off;
            if end < 64 {
                //shift already dropped `off` low bits of a partial word
                word &= (1 << (end - off)) - 1;
            }
            if word != 0 {
                return Some((w << 6) + off + word.trailing_zeros() as usize);
            }
            w += 1;
            off = 0;
        }
        None
    }

    ///last clear bit ≤ from (clamped to bits−1), or None
    #[inline]
    pub fn prev_clear(&self, from: usize) -> Option<usize> {
        if self.bits == 0 {
            return None;
        }
        let from = from.min(self.bits - 1);
        //fast path: `from` itself clear — it's the answer
        if self.words[from >> 6] & 1 << (from & 63) == 0 {
            return Some(from);
        }
        let mut w = from >> 6;
        let mut keep = from & 63;
        loop {
            let end = (self.bits - (w << 6)).min(64);
            let mut word = !self.words[w];
            word &= if keep == 63 { u64::MAX } else { (1 << (keep + 1)) - 1 };
            if end < 64 {
                word &= (1 << end) - 1;
            }
            if word != 0 {
                return Some((w << 6) + 63 - word.leading_zeros() as usize);
            }
            if w == 0 {
                return None;
            }
            w -= 1;
            keep = 63;
        }
    }

    ///nearest clear bit to `center`, ties resolved to the after side.
    ///`center` itself is excluded (callers pass a live position — the
    ///search starts at center±1)
    #[inline]
    pub fn nearest_clear(&self, center: usize) -> Option<usize> {
        //center < bits guard: next_clear treats ≥ bits as None anyway,
        //this also keeps center+1 from overflowing
        let after = if center < self.bits {
            self.next_clear(center + 1)
        } else {
            None
        };
        let before = center.checked_sub(1).and_then(|b| self.prev_clear(b));
        match (after, before) {
            (Some(a), Some(b)) => Some(if a - center <= center - b { a } else { b }),
            (a, b) => a.or(b),
        }
    }

    ///set bits in ascending order
    pub fn iter_set(&self) -> SetBits<'_> {
        let mut words = self.words.iter();
        SetBits { word: words.next().copied().unwrap_or(0), words, pos: 0 }
    }

    ///set bits in descending order
    pub fn iter_set_rev(&self) -> SetBitsRev<'_> {
        let mut words = self.words.iter().rev();
        SetBitsRev { word: words.next().copied().unwrap_or(0), words, pos: self.bits.wrapping_sub(1) }
    }

    ///the rotation turn's mask half: bit p → 2·p ± the band ripple. the
    ///band = positions whose names sit in the upper half of the name space
    ///— they land odd (2·p+1 forward, 2·p−1 reversed); everything else
    ///lands even. forward ramp `(p − offset) mod 2·midpoint` wraps at
    ///p = offset: at most one junction falls inside the range, so the band
    ///is one prefix (offset ≤ midpoint: ripple `[0, 2·min(offset,cap))`
    ///up), one suffix (offset > midpoint: `[2·(offset−midpoint), 2·cap)`
    ///up), whole-range, or empty. reversed ramp `offset − p` wraps at
    ///p = offset the other way: the upper half is the suffix `(offset,
    ///cap)` (offset < cap ≤ midpoint in every reachable state) — ripple
    ///`[2·offset+1, 2·cap)` DOWN.
    ///the up-ripple never crosses a word boundary (even bit → adjacent
    ///odd bit); the down-ripple does — a word's bit-0 element lands on the
    ///word below's bit 63 — so the fall carries word by word.
    ///panics if bits > midpoint (or, reversed, offset ≥ midpoint). set
    ///bits are preserved; the word count stays exact for the doubled
    ///range.
    pub fn rotate_mask(&mut self, offset: usize, midpoint: usize, reversed: bool) {
        assert!(self.bits <= midpoint, "bits {} > midpoint {midpoint}", self.bits);
        let old_words = self.words.len();
        let cap = self.bits;
        self.bits = cap * 2;
        self.words.resize(old_words * 2, 0);
        for i in (0..old_words).rev() {
            let w = self.words[i];
            self.words[i * 2] = spread(w as u32);
            self.words[i * 2 + 1] = spread((w >> 32) as u32);
        }
        self.words.truncate((self.bits + 63) / 64);
        //band target range + shift direction
        let (lo, hi, down) = if reversed {
            assert!(offset < midpoint, "reversed offset {offset} ≥ midpoint {midpoint}");
            (offset * 2 + 1, cap * 2, true)
        } else if offset <= midpoint {
            (0, offset.min(cap) * 2, false)
        } else {
            match offset - midpoint {
                //band beyond the range — plain interleave
                s if s < cap => (s * 2, cap * 2, false),
                _ => (1, 0, false),
            }
        };
        if lo >= hi {
            return;
        }
        let wlo = lo >> 6;
        let whi = (hi - 1) >> 6;
        //down: the word above's bit-0 element falls onto this word's bit 63
        let mut fall = 0u64;
        for wi in (wlo..=whi).rev() {
            let low = if wi == wlo { lo & 63 } else { 0 };
            let high = if wi == whi { ((hi - 1) & 63) + 1 } else { 64 };
            let rmask = if high == 64 {
                if low == 0 { u64::MAX } else { u64::MAX << low }
            } else {
                ((1u64 << (high - low)) - 1) << low
            };
            let w = self.words[wi];
            let ripple = if down {
                (w & rmask) >> 1 & rmask
            } else {
                (w & rmask) << 1 & rmask
            };
            let mut nw = ripple | w & !rmask;
            if fall != 0 {
                nw |= 1 << 63;
            }
            self.words[wi] = nw;
            fall = if down { w & rmask & 1 } else { 0 };
        }
    }

    ///bit p ⇔ old bit bits−1−p (the array-reversal half of a block
    ///reversal; same set, mirrored order)
    pub fn reverse(&mut self) {
        self.words.reverse();
        for w in &mut self.words {
            *w = w.reverse_bits();
        }
        //the old tail word's zero padding flipped into the low bits — shift
        //the array back down
        let slack = (64 - self.bits % 64) % 64;
        if slack != 0 {
            let mut below = 0;
            for w in self.words.iter_mut().rev() {
                let cur = *w;
                *w = (cur >> slack) | (below << (64 - slack));
                below = cur & ((1 << slack) - 1);
            }
        }
    }

    ///number of set bits
    pub fn count(&self) -> usize {
        self.words.iter().map(|w| w.count_ones() as usize).sum()
    }

    pub fn clear(&mut self) {
        self.words.iter_mut().for_each(|w| *w = 0);
    }
}

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

impl Iterator for SetBits<'_> {
    type Item = usize;

    #[inline]
    fn next(&mut self) -> Option<usize> {
        loop {
            if self.word & 1 << (self.pos & 63) != 0 {
                let p = self.pos;
                self.pos += 1;
                if self.pos & 63 == 0 {
                    self.word = self.words.next().copied().unwrap_or(0);
                }
                return Some(p);
            }
            //hole: advance one bit (tz-jumping measured slower — the jump
            //costs more than the retest for scattered holes)
            if self.word >> (self.pos & 63) == 0 {
                //nothing set at pos or beyond — the word is done. zero the
                //cache on exhaustion: `?` alone leaves the stale word and
                //a re-poll yields out-of-range positions
                self.pos = (self.pos | 63) + 1;
                match self.words.next() {
                    Some(&w) => self.word = w,
                    None => {
                        self.word = 0;
                        return None;
                    }
                }
            } else {
                self.pos += 1;
            }
        }
    }
}

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

impl Iterator for SetBitsRev<'_> {
    type Item = usize;

    #[inline]
    fn next(&mut self) -> Option<usize> {
        loop {
            let bit = 1u64 << (self.pos & 63);
            if self.word & bit != 0 {
                let p = self.pos;
                self.pos = self.pos.wrapping_sub(1);
                if self.pos & 63 == 63 {
                    self.word = self.words.next().copied().unwrap_or(0);
                }
                return Some(p);
            }
            //hole: advance one bit (see SetBits — tz-jumping measured slower)
            let below = self.word & (bit | bit.wrapping_sub(1));
            if below == 0 {
                //nothing set at pos or below — the word is done. zero the
                //cache on exhaustion (see SetBits): `?` alone leaves the
                //stale word and a re-poll yields out-of-range positions
                self.pos = (self.pos & !63).wrapping_sub(1);
                match self.words.next() {
                    Some(&w) => self.word = w,
                    None => {
                        self.word = 0;
                        return None;
                    }
                }
            } else {
                self.pos = self.pos.wrapping_sub(1);
            }
        }
    }
}

///bits of x moved to the even positions of a u64 (p → 2p)
fn spread(x: u32) -> u64 {
    let mut x = x as u64;
    x = (x | (x << 16)) & 0x0000_FFFF_0000_FFFF;
    x = (x | (x << 8)) & 0x00FF_00FF_00FF_00FF;
    x = (x | (x << 4)) & 0x0F0F_0F0F_0F0F_0F0F;
    x = (x | (x << 2)) & 0x3333_3333_3333_3333;
    x = (x | (x << 1)) & 0x5555_5555_5555_5555;
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mask_ones(bits: usize, set: &[usize]) -> Mask {
        let mut m = Mask::with_bits(bits);
        for p in set {
            m.set(*p, true);
        }
        m
    }

    #[test]
    fn scans_match_naive() {
        //dense 70% pattern over a range spanning several words + partial tail
        let bits = 200;
        let set: Vec<usize> = (0..bits).filter(|p| (p * 31 + 7) % 10 < 7).collect();
        let m = mask_ones(bits, &set);
        for from in 0..=bits {
            let naive_set = set.iter().copied().find(|p| *p >= from);
            assert_eq!(m.next_set(from), naive_set, "next_set from {from}");
            let naive_clear = (from..bits).find(|p| !set.contains(p));
            assert_eq!(m.next_clear(from), naive_clear, "next_clear from {from}");
            //prev_clear is inclusive of from (nearest_clear feeds it center−1)
            let naive_prev = (0..=from.min(bits - 1)).rev().find(|p| !set.contains(p));
            assert_eq!(m.prev_clear(from), naive_prev, "prev_clear from {from}");
            let naive_before = (0..from).rev().find(|p| !set.contains(p));
            //nearest_clear excludes `from` (callers pass a live position)
            let naive_after = (from + 1..bits).find(|p| !set.contains(p));
            let naive_near = match (naive_after, naive_before) {
                (Some(a), Some(b)) => Some(if a - from <= from - b { a } else { b }),
                (a, b) => a.or(b),
            };
            assert_eq!(m.nearest_clear(from), naive_near, "nearest_clear from {from}");
        }
        assert_eq!(m.last_set(), set.last().copied(), "last_set");
        for from in 0..=bits {
            let naive_prev_set = (0..from).rev().find(|p| set.contains(p));
            assert_eq!(m.prev_set(from), naive_prev_set, "prev_set from {from}");
        }
    }

    #[test]
    fn edges() {
        let mut m = Mask::new();
        assert_eq!(m.next_set(0), None);
        assert_eq!(m.next_clear(0), None);
        assert_eq!(m.last_set(), None);
        m.resize(64);
        assert_eq!(m.next_clear(0), Some(0));
        for p in 0..64 {
            m.set(p, true);
        }
        assert_eq!(m.next_clear(0), None);
        assert_eq!(m.next_set(0), Some(0));
        m.set(63, false);
        assert_eq!(m.next_clear(0), Some(63));
        assert_eq!(m.next_set(63), None);
        assert_eq!(m.next_set(62), Some(62));
        assert_eq!(m.count(), 63);
        m.resize(100);
        assert_eq!(m.get(63), false);
        assert_eq!(m.next_set(64), None);
        assert_eq!(m.next_clear(64), Some(64));
        m.set(99, true);
        assert_eq!(m.next_set(64), Some(99));
        m.set(99, false);
        m.clear();
        assert_eq!(m.count(), 0);
    }

    #[test]
    fn shrink_clears_tail() {
        let mut m = mask_ones(100, &[50, 99]);
        m.resize(64);
        assert_eq!(m.iter_set().collect::<Vec<_>>(), vec![50]);
        assert_eq!(m.count(), 1);
        //tail cleared: nothing past bits even after a re-grow
        m.resize(100);
        assert_eq!(m.iter_set().collect::<Vec<_>>(), vec![50]);
    }

    #[test]
    fn iter_set_ascending() {
        let m = mask_ones(300, &[0, 63, 64, 65, 127, 128, 299]);
        assert_eq!(m.iter_set().collect::<Vec<_>>(), vec![0, 63, 64, 65, 127, 128, 299]);
        assert_eq!(mask_ones(300, &[]).iter_set().next(), None);
    }

    #[test]
    fn iter_set_rev_descending() {
        let bits = 200;
        let set: Vec<usize> = (0..bits).filter(|p| (p * 31 + 7) % 10 < 7).collect();
        let m = mask_ones(bits, &set);
        let naive: Vec<usize> = set.iter().rev().copied().collect();
        assert_eq!(m.iter_set_rev().collect::<Vec<_>>(), naive);
        assert_eq!(mask_ones(300, &[]).iter_set_rev().next(), None);
    }

    ///poll twice past the end — exhaustion must clear the cached word, or
    ///a re-poll tests the stale word at boundary positions and yields
    ///out-of-range positions (→ panic through BlockSet::iter's key read)
    #[test]
    fn iterators_none_stable() {
        let asc = mask_ones(64, &[0]);
        let mut it = asc.iter_set();
        assert_eq!(it.next(), Some(0));
        assert_eq!(it.next(), None);
        assert_eq!(it.next(), None);
        let desc = mask_ones(64, &[63]);
        let mut it = desc.iter_set_rev();
        assert_eq!(it.next(), Some(63));
        assert_eq!(it.next(), None);
        assert_eq!(it.next(), None);
    }

    #[test]
    fn reverse_mirrors_bits() {
        for (bits, set) in [
            (0usize, &[][..]),
            (1, &[0][..]),
            (7, &[0, 6][..]),
            (64, &[0, 63][..]),
            (100, &[0, 1, 50, 99][..]),
            (128, &[64, 127][..]),
            (200, &[0, 63, 64, 65, 199][..]),
        ] {
            let mut m = mask_ones(bits, set);
            m.reverse();
            let mut naive: Vec<usize> = set.iter().map(|p| bits - 1 - p).collect();
            naive.sort_unstable();
            assert_eq!(m.iter_set().collect::<Vec<_>>(), naive, "bits {bits}");
            assert_eq!(m.count(), set.len());
            //double reversal = identity
            m.reverse();
            assert_eq!(m.iter_set().collect::<Vec<_>>(), set, "bits {bits} twice");
        }
    }

    #[test]
    fn rotate_mask_plain_doubles() {
        let mut m = mask_ones(100, &[0, 1, 63, 64, 99]);
        m.rotate_mask(0, 128, false);
        assert_eq!(m.bits(), 200);
        assert_eq!(m.iter_set().collect::<Vec<_>>(), vec![0, 2, 126, 128, 198]);
        assert_eq!(m.count(), 5);
        //doubled range still exact-scannable
        assert_eq!(m.next_clear(3), Some(3));
        //non-word-multiple range keeps words_for exact
        assert_eq!(m.words.len(), (200 + 63) / 64);
        let mut m = mask_ones(65, &[64]);
        m.rotate_mask(0, 128, false);
        assert_eq!(m.bits(), 130);
        assert_eq!(m.iter_set().collect::<Vec<_>>(), vec![128]);
        assert_eq!(m.words.len(), (130 + 63) / 64);
    }

    ///bit p → 2p + [(p − offset) mod name space ≥ midpoint] at every
    ///offset — prefix, suffix, whole-range and empty bands, word-edge and
    ///mid-word junctions
    #[test]
    fn rotate_mask_matches_naive() {
        let bits = 100;
        let mut set: Vec<usize> = (0..bits).filter(|p| (p * 31 + 7) % 10 < 7).collect();
        set.extend([0, 1, 63, 64, 65, 99]);
        set.sort_unstable();
        set.dedup();
        let (mid, space) = (128usize, 256usize);
        for offset in 0..=255 {
            let mut m = mask_ones(bits, &set);
            m.rotate_mask(offset, mid, false);
            let mut naive: Vec<usize> = set
                .iter()
                .map(|&p| 2 * p + usize::from((p + space - offset) % space >= mid))
                .collect();
            naive.sort_unstable();
            assert_eq!(m.iter_set().collect::<Vec<_>>(), naive, "offset {offset}");
            assert_eq!(m.count(), set.len(), "offset {offset}");
            assert_eq!(m.bits(), 2 * bits, "offset {offset}");
        }
    }

    ///reversed turn: bit p → 2p − [name ∈ upper half] — the upper-half
    ///names form the suffix past the wrap point (a2p(0) = offset), landing
    ///one slot DOWN. the down-ripple crosses word boundaries (a word's
    ///bit-0 element falls to the word below's bit 63), so word-edge cases
    ///get their own sweep. offset < bits ≤ midpoint in every reachable
    ///reversed state
    #[test]
    fn rotate_mask_reversed_matches_naive() {
        let (mid, space) = (128usize, 256usize);
        for bits in [1usize, 7, 64, 100, 128] {
            let mut set: Vec<usize> = (0..bits).filter(|p| (p * 31 + 7) % 10 < 7).collect();
            set.extend([0, 1, 62, 63, 64, 65, bits - 1].into_iter().filter(|p| *p < bits));
            set.sort_unstable();
            set.dedup();
            for offset in 0..mid {
                let mut m = mask_ones(bits, &set);
                m.rotate_mask(offset, mid, true);
                let mut naive: Vec<usize> = set
                    .iter()
                    .map(|&p| {
                        let upper = (offset + space - p) % space >= mid;
                        2 * p - usize::from(upper)
                    })
                    .collect();
                naive.sort_unstable();
                assert_eq!(m.iter_set().collect::<Vec<_>>(), naive, "bits {bits} off {offset}");
                assert_eq!(m.count(), set.len(), "bits {bits} off {offset}");
                assert_eq!(m.bits(), 2 * bits, "bits {bits} off {offset}");
            }
        }
    }
}

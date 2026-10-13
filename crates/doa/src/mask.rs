//!dense occupancy bitmap — bit p ⇔ slot p live. word-at-a-time scans
//!(`next_set`/`next_clear` skip full/empty words, no per-bit loop); bits past
//! `bits` read as clear, so `next_clear` never returns ≥ `bits`.

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
    pub fn next_set(&self, from: usize) -> Option<usize> {
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
    pub fn prev_set(&self, from: usize) -> Option<usize> {
        let from = from.min(self.bits);
        if from == 0 {
            return None;
        }
        let mut w = (from - 1) >> 6;
        let mut keep = ((from - 1) & 63) + 1;
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
    pub fn next_clear(&self, from: usize) -> Option<usize> {
        if from >= self.bits {
            return None;
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
    pub fn prev_clear(&self, from: usize) -> Option<usize> {
        if self.bits == 0 {
            return None;
        }
        let from = from.min(self.bits - 1);
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

    ///nearest clear bit to `center`, ties resolved to the after side
    pub fn nearest_clear(&self, center: usize) -> Option<usize> {
        let after = self.next_clear(center);
        let before = center.checked_sub(1).and_then(|b| self.prev_clear(b));
        match (after, before) {
            (Some(a), Some(b)) => Some(if a - center <= center - b { a } else { b }),
            (a, b) => a.or(b),
        }
    }

    ///set bits in ascending order
    pub fn iter_set(&self) -> impl Iterator<Item = usize> + '_ {
        let mut from = 0;
        std::iter::from_fn(move || {
            let n = self.next_set(from)?;
            from = n + 1;
            Some(n)
        })
    }

    ///the rotation turn's mask half: bit p → 2·p, +1 over the wrap band —
    ///the pos whose name-space ramp (p − offset) mod 2·midpoint sits in
    ///the upper half. the plain interleave opens a hole after every
    ///element; the band's ripple shifts its target range up one, so the
    ///band's low junction consumes that hole (the addr MAX|MIN pair —
    ///prefix, offset ≤ midpoint) or doubles it (suffix). at most one wrap
    ///event falls inside a range capped at midpoint, so the band is one
    ///prefix or suffix. the ripple never crosses a word boundary (even
    ///bit → adjacent odd bit), so each word shifts independently.
    ///panics if bits > midpoint. set bits are preserved; the word count
    ///stays exact for the doubled range.
    pub fn rotate_mask(&mut self, offset: usize, midpoint: usize) {
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
        //band target range: prefix [0, min(offset, cap)) lands odd, or
        //suffix [offset − midpoint, cap) when the window sits past midpoint
        let (lo, hi) = if offset <= midpoint {
            (0, offset.min(cap) * 2)
        } else {
            match offset - midpoint {
                s if s < cap => (s * 2, cap * 2),
                _ => (1, 0), //band beyond the range — plain interleave
            }
        };
        if lo >= hi {
            return;
        }
        let wlo = lo >> 6;
        let whi = (hi - 1) >> 6;
        for wi in wlo..=whi {
            let low = if wi == wlo { lo & 63 } else { 0 };
            let high = if wi == whi { ((hi - 1) & 63) + 1 } else { 64 };
            let rmask = if high == 64 {
                if low == 0 { u64::MAX } else { u64::MAX << low }
            } else {
                ((1u64 << (high - low)) - 1) << low
            };
            let w = self.words[wi];
            self.words[wi] = (w & rmask) << 1 & rmask | w & !rmask;
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
            let naive_near = match (naive_clear, naive_before) {
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
    fn rotate_mask_plain_doubles() {
        let mut m = mask_ones(100, &[0, 1, 63, 64, 99]);
        m.rotate_mask(0, 128);
        assert_eq!(m.bits(), 200);
        assert_eq!(m.iter_set().collect::<Vec<_>>(), vec![0, 2, 126, 128, 198]);
        assert_eq!(m.count(), 5);
        //doubled range still exact-scannable
        assert_eq!(m.next_clear(3), Some(3));
        //non-word-multiple range keeps words_for exact
        assert_eq!(m.words.len(), (200 + 63) / 64);
        let mut m = mask_ones(65, &[64]);
        m.rotate_mask(0, 128);
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
            m.rotate_mask(offset, mid);
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
}

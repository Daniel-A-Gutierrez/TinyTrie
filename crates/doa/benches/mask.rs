//!mask word width: the library `Mask` (u64) vs u32/u128 ports of the SAME
//! algorithms — `GMask` mirrors src/mask.rs exactly (fast paths, the
//! advance-one-bit hole policy, the None-stable word cache). ops: iter_set
//! (ns/bit), iter_set_rev (ns/bit), next_set + next_clear from
//! scan-forcing positions (ns/query). 3 shapes (dense 7/8 strided holes,
//! scattered 70%, runs 128/24) × 2 sizes: 4096 bits (512B, L1) and 1Mi bits
//! (128KB, L2) — same BYTES at every width, so width only changes word
//! count + per-word op cost. every port is verified against the library
//! mask on every shape before timing.

use doa::mask::Mask;
use std::hint::black_box;
use std::time::Instant;

const SMALL: usize = 4096;
const BIG: usize = 1 << 20;
///query sample cap — keeps BIG sweeps probe-like (distinct cache lines)
const Q_CAP: usize = 4096;
const REPS: usize = 7;

#[derive(Clone, Copy, Debug)]
enum Shape {
    Dense,
    Scattered,
    Runs,
}

impl Shape {
    fn name(self) -> &'static str {
        match self {
            Shape::Dense => "dense",
            Shape::Scattered => "scattered",
            Shape::Runs => "runs",
        }
    }

    fn live(self, p: usize) -> bool {
        match self {
            Shape::Dense => p % 8 != 5,
            Shape::Scattered => (p * 31 + 7) % 10 < 7,
            Shape::Runs => p % 152 < 128,
        }
    }
}

//---- generic word ----------------------------------------------------------

trait Word: Copy + PartialEq {
    const SHIFT: u32;
    const WIDTH: usize;
    const ZERO: Self;
    fn bit(off: usize) -> Self;
    fn tz(self) -> u32;
    fn not(self) -> Self;
    fn and(self, o: Self) -> Self;
    fn or(self, o: Self) -> Self;
    fn shr(self, off: usize) -> Self;
    ///bits [0, k) set — k ≥ 1
    fn low_mask(k: usize) -> Self;
    fn wrapping_dec(self) -> Self;
}

macro_rules! word_impl {
    ($t:ty, $shift:expr) => {
        impl Word for $t {
            const SHIFT: u32 = $shift;
            const WIDTH: usize = 1 << $shift;
            const ZERO: Self = 0;
            #[inline(always)]
            fn bit(off: usize) -> Self {
                1 << off
            }
            #[inline(always)]
            fn tz(self) -> u32 {
                self.trailing_zeros()
            }
            #[inline(always)]
            fn not(self) -> Self {
                !self
            }
            #[inline(always)]
            fn and(self, o: Self) -> Self {
                self & o
            }
            #[inline(always)]
            fn or(self, o: Self) -> Self {
                self | o
            }
            #[inline(always)]
            fn shr(self, off: usize) -> Self {
                self >> off
            }
            #[inline(always)]
            fn low_mask(k: usize) -> Self {
                Self::MAX >> (Self::WIDTH - k)
            }
            #[inline(always)]
            fn wrapping_dec(self) -> Self {
                self.wrapping_sub(1)
            }
        }
    };
}

word_impl!(u32, 5);
word_impl!(u64, 6);
word_impl!(u128, 7);

//---- GMask: the src/mask.rs algorithms, width-generic ----------------------

struct GMask<W: Word> {
    words: Vec<W>,
    bits:  usize,
}

impl<W: Word> GMask<W> {
    fn build(bits: usize, shape: Shape) -> Self {
        let mut words = vec![W::ZERO; (bits + W::WIDTH - 1) / W::WIDTH];
        for p in 0..bits {
            if shape.live(p) {
                words[p >> W::SHIFT] = words[p >> W::SHIFT].or(W::bit(p & (W::WIDTH - 1)));
            }
        }
        GMask { words, bits }
    }

    ///mirrors `Mask::next_set` — dense fast path, then the word scan
    fn next_set(&self, from: usize) -> Option<usize> {
        let b = W::WIDTH;
        let mut w = from >> W::SHIFT;
        let mut off = from & (b - 1);
        if w < self.words.len() && self.words[w].and(W::bit(off)) != W::ZERO {
            return Some(from);
        }
        while w < self.words.len() {
            let word = self.words[w].shr(off);
            if word != W::ZERO {
                return Some((w << W::SHIFT) + off + word.tz() as usize);
            }
            w += 1;
            off = 0;
        }
        None
    }

    ///mirrors `Mask::next_clear` — fast path, then the negated word scan
    ///with the partial-tail mask
    fn next_clear(&self, from: usize) -> Option<usize> {
        if from >= self.bits {
            return None;
        }
        let b = W::WIDTH;
        if self.words[from >> W::SHIFT].and(W::bit(from & (b - 1))) == W::ZERO {
            return Some(from);
        }
        let mut w = from >> W::SHIFT;
        let mut off = from & (b - 1);
        while w < self.words.len() {
            let end = (self.bits - (w << W::SHIFT)).min(b);
            let mut word = self.words[w].not().shr(off);
            if end < b {
                word = word.and(W::low_mask(end - off));
            }
            if word != W::ZERO {
                return Some((w << W::SHIFT) + off + word.tz() as usize);
            }
            w += 1;
            off = 0;
        }
        None
    }

    fn iter_set(&self) -> GSetBits<'_, W> {
        let mut words = self.words.iter();
        GSetBits { word: words.next().copied().unwrap_or(W::ZERO), words, pos: 0 }
    }

    fn iter_set_rev(&self) -> GSetBitsRev<'_, W> {
        let mut words = self.words.iter().rev();
        GSetBitsRev { word: words.next().copied().unwrap_or(W::ZERO), words, pos: self.bits.wrapping_sub(1) }
    }
}

///mirrors `SetBits` — cached word, advance-one-bit on holes, None-stable
struct GSetBits<'a, W: Word> {
    words: std::slice::Iter<'a, W>,
    word:  W,
    pos:   usize,
}

impl<W: Word> Iterator for GSetBits<'_, W> {
    type Item = usize;

    #[inline]
    fn next(&mut self) -> Option<usize> {
        let b = W::WIDTH;
        loop {
            let off = self.pos & (b - 1);
            if self.word.and(W::bit(off)) != W::ZERO {
                let p = self.pos;
                self.pos += 1;
                if self.pos & (b - 1) == 0 {
                    self.word = self.words.next().copied().unwrap_or(W::ZERO);
                }
                return Some(p);
            }
            if self.word.shr(off) == W::ZERO {
                self.pos = (self.pos | (b - 1)) + 1;
                match self.words.next() {
                    Some(&w) => self.word = w,
                    None => {
                        self.word = W::ZERO;
                        return None;
                    }
                }
            } else {
                self.pos += 1;
            }
        }
    }
}

///mirrors `SetBitsRev` — the descending mirror
struct GSetBitsRev<'a, W: Word> {
    words: std::iter::Rev<std::slice::Iter<'a, W>>,
    word:  W,
    pos:   usize,
}

impl<W: Word> Iterator for GSetBitsRev<'_, W> {
    type Item = usize;

    #[inline]
    fn next(&mut self) -> Option<usize> {
        let b = W::WIDTH;
        loop {
            let bit = W::bit(self.pos & (b - 1));
            if self.word.and(bit) != W::ZERO {
                let p = self.pos;
                self.pos = self.pos.wrapping_sub(1);
                if self.pos & (b - 1) == b - 1 {
                    self.word = self.words.next().copied().unwrap_or(W::ZERO);
                }
                return Some(p);
            }
            //bits [0..=off] — the word is done at or below pos
            if self.word.and(bit.or(bit.wrapping_dec())) == W::ZERO {
                self.pos = (self.pos & !(b - 1)).wrapping_sub(1);
                match self.words.next() {
                    Some(&w) => self.word = w,
                    None => {
                        self.word = W::ZERO;
                        return None;
                    }
                }
            } else {
                self.pos = self.pos.wrapping_sub(1);
            }
        }
    }
}

//---- bench surface: real Mask + GMask behind one trait ----------------------

trait Bench {
    type Fwd<'a>: Iterator<Item = usize>
    where
        Self: 'a;
    type Rev<'a>: Iterator<Item = usize>
    where
        Self: 'a;
    fn b_next_set(&self, from: usize) -> Option<usize>;
    fn b_next_clear(&self, from: usize) -> Option<usize>;
    fn b_iter(&self) -> Self::Fwd<'_>;
    fn b_iter_rev(&self) -> Self::Rev<'_>;
}

impl Bench for Mask {
    type Fwd<'a> = doa::mask::SetBits<'a>;
    type Rev<'a> = doa::mask::SetBitsRev<'a>;
    fn b_next_set(&self, from: usize) -> Option<usize> {
        self.next_set(from)
    }
    fn b_next_clear(&self, from: usize) -> Option<usize> {
        self.next_clear(from)
    }
    fn b_iter(&self) -> Self::Fwd<'_> {
        self.iter_set()
    }
    fn b_iter_rev(&self) -> Self::Rev<'_> {
        self.iter_set_rev()
    }
}

impl<W: Word> Bench for GMask<W> {
    type Fwd<'a> = GSetBits<'a, W> where W: 'a;
    type Rev<'a> = GSetBitsRev<'a, W> where W: 'a;
    fn b_next_set(&self, from: usize) -> Option<usize> {
        self.next_set(from)
    }
    fn b_next_clear(&self, from: usize) -> Option<usize> {
        self.next_clear(from)
    }
    fn b_iter(&self) -> Self::Fwd<'_> {
        self.iter_set()
    }
    fn b_iter_rev(&self) -> Self::Rev<'_> {
        self.iter_set_rev()
    }
}

//---- verify: every port matches the library mask ---------------------------

fn verify(shape: Shape, bits: usize, real: &Mask, g32: &GMask<u32>, g64: &GMask<u64>, g128: &GMask<u128>) {
    let check = |tag: &str, p: usize, got: Option<usize>, want: Option<usize>| {
        assert_eq!(got, want, "{tag} mismatch at {p} ({}, {} bits)", shape.name(), bits);
    };
    //full iter equality both directions, per port vs the real engine
    macro_rules! port_rows {
        ($m:expr, $tag:expr) => {{
            let m = $m;
            assert!(real.iter_set().zip(m.iter_set()).all(|(a, b)| a == b), "iter {} {} {}", $tag, shape.name(), bits);
            assert!(real.iter_set_rev().zip(m.iter_set_rev()).all(|(a, b)| a == b), "iter_rev {} {} {}", $tag, shape.name(), bits);
        }};
    }
    port_rows!(g32, "u32");
    port_rows!(g64, "u64");
    port_rows!(g128, "u128");
    //scans: every position when small, stride 97 when big
    let stride = if bits <= SMALL { 1 } else { 97 };
    for p in (0..=bits).step_by(stride) {
        check("next_set", p, g32.next_set(p), real.next_set(p));
        check("next_set", p, g64.next_set(p), real.next_set(p));
        check("next_set", p, g128.next_set(p), real.next_set(p));
        check("next_clear", p, g32.next_clear(p), real.next_clear(p));
        check("next_clear", p, g64.next_clear(p), real.next_clear(p));
        check("next_clear", p, g128.next_clear(p), real.next_clear(p));
    }
}

//---- timing ---------------------------------------------------------------

///best-of-REPS ns/bit for a full forward pass
fn time_iter<M: Bench>(m: &M, bits: usize, live: usize, rev: bool) -> f64 {
    let passes = (4_000_000 / bits).max(2);
    let mut best = f64::MAX;
    for _ in 0..REPS {
        let t = Instant::now();
        let mut acc = 0usize;
        for _ in 0..passes {
            if rev {
                for p in m.b_iter_rev() {
                    acc ^= p;
                }
            } else {
                for p in m.b_iter() {
                    acc ^= p;
                }
            }
        }
        black_box(acc);
        best = best.min(t.elapsed().as_nanos() as f64 / (passes * live) as f64);
    }
    best
}

///best-of-REPS ns/query — one sweep = every query once
fn time_scan<M: Bench>(m: &M, qs: &[usize], clear: bool) -> f64 {
    let sweeps = (1_500_000 / qs.len()).max(2);
    let mut best = f64::MAX;
    for _ in 0..REPS {
        let t = Instant::now();
        let mut acc = 0usize;
        for _ in 0..sweeps {
            for &q in qs {
                acc ^= if clear { m.b_next_clear(q) } else { m.b_next_set(q) }.unwrap_or(0);
            }
        }
        black_box(acc);
        best = best.min(t.elapsed().as_nanos() as f64 / (sweeps * qs.len()) as f64);
    }
    best
}

///sample ≤ Q_CAP positions from `all` — even stride
fn sample(all: &[usize]) -> Vec<usize> {
    if all.len() <= Q_CAP {
        all.to_vec()
    } else {
        all.iter().step_by(all.len() / Q_CAP).copied().collect()
    }
}

fn main() {
    for bits in [SMALL, BIG] {
        for shape in [Shape::Dense, Shape::Scattered, Shape::Runs] {
            let mut real = Mask::with_bits(bits);
            let mut holes = Vec::new();
            let mut live = Vec::new();
            for p in 0..bits {
                if shape.live(p) {
                    real.set(p, true);
                    live.push(p);
                } else {
                    holes.push(p);
                }
            }
            let g32 = GMask::<u32>::build(bits, shape);
            let g64 = GMask::<u64>::build(bits, shape);
            let g128 = GMask::<u128>::build(bits, shape);
            verify(shape, bits, &real, &g32, &g64, &g128);
            //next_set from holes (fast path misses, scan forced); next_clear
            //from live (fast path misses, scan to the next hole)
            let set_q = sample(&holes);
            let clear_q = sample(&live);
            println!("\n{} {} bits (live {}, words {}/{}/{}):", shape.name(), bits, live.len(), g32.words.len(), g64.words.len(), g128.words.len());
            let row = |label: &str, cells: [f64; 4]| {
                println!(
                    "  {label:<10} {:>7.3} {:>7.3} {:>7.3} {:>7.3}",
                    cells[0], cells[1], cells[2], cells[3]
                );
            };
            row("iter", [
                time_iter(&real, bits, live.len(), false),
                time_iter(&g32, bits, live.len(), false),
                time_iter(&g64, bits, live.len(), false),
                time_iter(&g128, bits, live.len(), false),
            ]);
            row("iter_rev", [
                time_iter(&real, bits, live.len(), true),
                time_iter(&g32, bits, live.len(), true),
                time_iter(&g64, bits, live.len(), true),
                time_iter(&g128, bits, live.len(), true),
            ]);
            row("next_set", [
                time_scan(&real, &set_q, false),
                time_scan(&g32, &set_q, false),
                time_scan(&g64, &set_q, false),
                time_scan(&g128, &set_q, false),
            ]);
            row("next_clear", [
                time_scan(&real, &clear_q, true),
                time_scan(&g32, &clear_q, true),
                time_scan(&g64, &clear_q, true),
                time_scan(&g128, &clear_q, true),
            ]);
        }
    }
    println!("\ncolumns: Mask(u64) | u32 | u64 | u128   — iter/iter_rev ns/bit, next_set/next_clear ns/query");
}
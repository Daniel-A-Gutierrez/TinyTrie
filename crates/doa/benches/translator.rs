//!dispatch A/B: the OLD fn-ptr-per-direction Translator (dropped 2026-10-23 —
//!a serialized/memmapped translator would carry a code address, a hijack
//!primitive; also measured ~4 cycles/call slower) vs the library's inline
//!`if reversed` branch applying rotation+offset unconditionally. kept as the
//!record of why the branch shape is in the library. ops: p2a + a2p over a
//!random probe stream (u32 addrs, 2MB stream), rotation 0 and 13, forward
//!and reversed, best-of-REPS min ns/call. the two shapes are asserted equal
//!on every cell before timing. a pure-load row floors the loop+stream cost.

use std::hint::black_box;
use std::time::Instant;

const STREAM: usize = 1 << 18;
const REPS: usize = 10;

//---- the library shape (src/translator.rs verbatim math) -------------------

fn a2p_fwd(a: u32, rotation: u32, offset: u32) -> u32 {
    a.rotate_left(rotation).wrapping_add(offset)
}

fn a2p_rev(a: u32, rotation: u32, offset: u32) -> u32 {
    offset.wrapping_sub(a.rotate_left(rotation))
}

fn p2a_fwd(p: usize, rotation: u32, offset: u32) -> u32 {
    (p as u32).wrapping_sub(offset).rotate_right(rotation)
}

fn p2a_rev(p: usize, rotation: u32, offset: u32) -> u32 {
    offset.wrapping_sub(p as u32).rotate_right(rotation)
}

#[derive(Clone, Copy)]
struct FnPtrTr {
    rotation: u32,
    offset:   u32,
    //kept to mirror the library struct (its `reversed` getter reads it)
    #[allow(dead_code)]
    reversed: bool,
    a2p:      fn(u32, u32, u32) -> u32,
    p2a:      fn(usize, u32, u32) -> u32,
}

impl FnPtrTr {
    fn new(rotation: u32, offset: u32, reversed: bool) -> Self {
        FnPtrTr {
            rotation,
            offset,
            reversed,
            a2p: if reversed { a2p_rev } else { a2p_fwd },
            p2a: if reversed { p2a_rev } else { p2a_fwd },
        }
    }

    #[inline]
    fn a2p(&self, a: u32) -> u32 {
        (self.a2p)(a, self.rotation, self.offset)
    }

    #[inline]
    fn p2a(&self, p: usize) -> u32 {
        (self.p2a)(p, self.rotation, self.offset)
    }
}

//---- the branch shape: one path, rot+offset unconditional ------------------

#[derive(Clone, Copy)]
struct BranchTr {
    rotation: u32,
    offset:   u32,
    reversed: bool,
}

impl BranchTr {
    #[inline(always)]
    fn a2p(&self, a: u32) -> u32 {
        let t = a.rotate_left(self.rotation);
        if self.reversed { self.offset.wrapping_sub(t) } else { t.wrapping_add(self.offset) }
    }

    #[inline(always)]
    fn p2a(&self, p: usize) -> u32 {
        let x = if self.reversed {
            self.offset.wrapping_sub(p as u32)
        } else {
            (p as u32).wrapping_sub(self.offset)
        };
        x.rotate_right(self.rotation)
    }
}

//---- timing ----------------------------------------------------------------

macro_rules! bench {
    ($label:expr, $tr:expr, $stream:expr, $op:ident) => {{
        let tr = black_box($tr);
        let mut best = f64::MAX;
        for _ in 0..REPS {
            let start = Instant::now();
            let mut acc = 0u32;
            for &x in $stream {
                acc = acc.wrapping_add(tr.$op(x));
            }
            black_box(acc);
            best = best.min(start.elapsed().as_nanos() as f64 / $stream.len() as f64);
        }
        println!("{:<28} {:>7.3} ns/call", $label, best);
    }};
}

fn main() {
    //xorshift probe streams
    let mut seed = 0x9E3779B97F4A7C15u64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let positions: Vec<usize> =
        (0..STREAM).map(|_| (next() as usize) % (1 << 20)).collect();
    let names: Vec<u32> = (0..STREAM).map(|_| next() as u32).collect();

    //equivalence sweep: every cell must agree before it's timed
    for &rotation in &[0u32, 13] {
        for &reversed in &[false, true] {
            let f = FnPtrTr::new(rotation, 12345, reversed);
            let b = BranchTr { rotation, offset: 12345, reversed };
            for p in (0..(1 << 16)).step_by(7) {
                assert_eq!(f.p2a(p), b.p2a(p), "p2a r={rotation} rev={reversed} p={p}");
            }
            for i in 0..4096 {
                let a = names[i];
                assert_eq!(f.a2p(a), b.a2p(a), "a2p r={rotation} rev={reversed}");
            }
        }
    }

    //floor: the loop + stream load, no translation
    let mut best = f64::MAX;
    for _ in 0..REPS {
        let start = Instant::now();
        let mut acc = 0u32;
        for &p in &positions {
            acc = acc.wrapping_add(p as u32);
        }
        black_box(acc);
        best = best.min(start.elapsed().as_nanos() as f64 / STREAM as f64);
    }
    println!("{:<28} {:>7.3} ns/call", "load floor (p2a stream)", best);

    for &rotation in &[0u32, 13] {
        for &reversed in &[false, true] {
            let dir = if reversed { "rev" } else { "fwd" };
            let f = FnPtrTr::new(rotation, 12345, reversed);
            let b = BranchTr { rotation, offset: 12345, reversed };
            println!("--- rot={rotation} {dir} ---");
            bench!(format!("fn-ptr p2a {dir}"), f, &positions, p2a);
            bench!(format!("branch p2a {dir}"), b, &positions, p2a);
            bench!(format!("fn-ptr a2p {dir}"), f, &names, a2p);
            bench!(format!("branch a2p {dir}"), b, &names, a2p);
        }
    }
}
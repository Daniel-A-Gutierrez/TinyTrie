//! Microbench: CircularArray<T, i32> vs VecDeque<T>.
//!
//! Apples-to-apples on the operations both support: push_back, push_front,
//! mixed two-ended push, forward/reverse iteration, and indexed get. Run with
//! `cargo run --release --example bench_ca`.
//!
//! Stored values are runtime-random (seed from `Instant::now`) so the compiler
//! cannot constant-fold a sum of known values. `CircularArray` requires
//! `T: Clone`; `i32: Clone` so the comparison is fair. For a push_back-only
//! build, CircularArray address n == VecDeque index n, so the `get` comparison
//! uses the identical access pattern.

use std::collections::VecDeque;
use std::hint::black_box;
use std::time::{Duration, Instant};

use doa::CircularArray;

type CA = CircularArray<i32, i32>;

const N: usize = 1_000_000;
const RUNS: usize = 5; // min over runs to reduce noise

/// Runtime-random stored values: defeat constant-folding of any sum.
fn values() -> Vec<i32> {
    let mut s = Instant::now().elapsed().as_nanos() as u64 | 1;
    (0..N)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s as i32
        })
        .collect()
}

fn time<R>(f: impl Fn() -> R) -> Duration {
    let mut best = Duration::MAX;
    for _ in 0..RUNS {
        let t = Instant::now();
        let r = f();
        best = best.min(t.elapsed());
        black_box(r);
    }
    best
}

fn fmt(ns_per_op: f64) -> String {
    if ns_per_op >= 1000.0 {
        format!("{:7.2} us/op", ns_per_op / 1000.0)
    } else {
        format!("{:7.2} ns/op", ns_per_op)
    }
}

fn main() {
    let vals = values();
    println!("N = {N} elements (i32, runtime-random values), min of {RUNS} runs\n");

    let row = |label: &str, ca: Duration, vd: Duration| {
        let ca_ns = ca.as_nanos() as f64 / N as f64;
        let vd_ns = vd.as_nanos() as f64 / N as f64;
        let ratio = ca_ns / vd_ns;
        println!(
            "{:<22} CA {:>14}   VD {:>14}   ratio {:>5.2}x",
            label,
            fmt(ca_ns),
            fmt(vd_ns),
            ratio
        );
    };

    // ---- push_back ------------------------------------------------------
    let t = time(|| {
        let mut c = CA::new();
        for &v in &vals {
            c.push_back(v);
        }
        c
    });
    let v = time(|| {
        let mut d: VecDeque<i32> = VecDeque::with_capacity(N);
        for &v in &vals {
            d.push_back(v);
        }
        d
    });
    row("push_back", t, v);

    // ---- push_front -----------------------------------------------------
    let t = time(|| {
        let mut c = CA::new();
        for &v in &vals {
            c.push_front(v);
        }
        c
    });
    let v = time(|| {
        let mut d: VecDeque<i32> = VecDeque::with_capacity(N);
        for &v in &vals {
            d.push_front(v);
        }
        d
    });
    row("push_front", t, v);

    // ---- mixed (interleave back/front) ----------------------------------
    let t = time(|| {
        let mut c = CA::new();
        for (i, &v) in vals.iter().enumerate() {
            if i & 1 == 0 {
                c.push_back(v);
            } else {
                c.push_front(v);
            }
        }
        c
    });
    let v = time(|| {
        let mut d: VecDeque<i32> = VecDeque::with_capacity(N);
        for (i, &v) in vals.iter().enumerate() {
            if i & 1 == 0 {
                d.push_back(v);
            } else {
                d.push_front(v);
            }
        }
        d
    });
    row("push mixed", t, v);

    // ---- build once, then time iteration alone --------------------------
    let mut c = CA::new();
    for &v in &vals {
        c.push_back(v);
    }
    let mut d: VecDeque<i32> = VecDeque::with_capacity(N);
    for &v in &vals {
        d.push_back(v);
    }
    assert_eq!(c.len(), N);
    assert_eq!(d.len(), N);

    let t = time(|| {
        let mut sum = 0i64;
        for x in c.iter() {
            sum += *x as i64;
        }
        sum
    });
    let v = time(|| {
        let mut sum = 0i64;
        for x in d.iter() {
            sum += *x as i64;
        }
        sum
    });
    row("fwd iter (sum)", t, v);

    let t = time(|| {
        let mut sum = 0i64;
        for x in c.rev_iter() {
            sum += *x as i64;
        }
        sum
    });
    let v = time(|| {
        let mut sum = 0i64;
        for x in d.iter().rev() {
            sum += *x as i64;
        }
        sum
    });
    row("rev iter (sum)", t, v);

    // ---- indexed get: identical pseudo-random access pattern -----------
    let indices: Vec<usize> = {
        let mut s = 0x9e3779b97f4a7c15u64;
        (0..N)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                (s as usize) % N
            })
            .collect()
    };
    let addr = |n: usize| n as i32; // push_back-only build: address n == index n

    let t = time(|| {
        let mut sum = 0i64;
        for &i in &indices {
            sum += *c.get(addr(i)).unwrap() as i64;
        }
        sum
    });
    let v = time(|| {
        let mut sum = 0i64;
        for &i in &indices {
            sum += d[i] as i64;
        }
        sum
    });
    row("indexed get (random)", t, v);

    // ---- sequential get (cache-friendly, isolates per-element overhead) -
    let t = time(|| {
        let mut sum = 0i64;
        for i in 0..N {
            sum += *c.get(addr(i)).unwrap() as i64;
        }
        sum
    });
    let v = time(|| {
        let mut sum = 0i64;
        for i in 0..N {
            sum += d[i] as i64;
        }
        sum
    });
    row("indexed get (seq)", t, v);

    // ---- memory (allocated bytes for the built structure) --------------
    let ca_bytes = std::mem::size_of_val(&c) + (c.lens().0 + c.lens().1) * std::mem::size_of::<i32>();
    let vd_cap = d.capacity();
    let vd_bytes = std::mem::size_of::<VecDeque<i32>>() + vd_cap * std::mem::size_of::<i32>();
    println!(
        "\nmem (struct + live slots):  CA {:>8} B   VD {:>8} B",
        ca_bytes, vd_bytes
    );
}
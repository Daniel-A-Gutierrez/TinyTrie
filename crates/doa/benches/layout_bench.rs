#![feature(test)]
#![feature(portable_simd)]
//! AoS vs SoA K->V map lookup: `[(K,V); N]` vs `struct { keys: [K; N], vals: [V; N] }`.
//! N = 8 and 16. K = u64 (inline) and String (out of line).
//! Probes a *missing* key so every bench does a full N-scan — isolates the
//! layout/cache cost from hit-position luck.
//!
//! Run: `cargo bench -p doa --bench layout_bench`

extern crate test;

use std::simd::cmp::SimdPartialEq;
use std::simd::{u64x16, u64x4, u64x8};
use test::{Bencher, black_box};

// ---- AoS: array of (K, V) tuples ----

fn lookup_aos<'a, K, V, const N: usize>(arr: &'a [(K, V); N], key: &K) -> Option<&'a V>
where
    K: Eq,
{
    for (k, v) in arr {
        if k == key { return Some(v); }
    }
    None
}

// ---- SoA: split keys / vals ----

#[derive(Clone)]
struct Soa<K, V, const N: usize> {
    keys: [K; N],
    vals: [V; N],
}

fn lookup_soa<'a, K, V, const N: usize>(s: &'a Soa<K, V, N>, key: &K) -> Option<&'a V>
where
    K: Eq,
{
    for i in 0..N {
        if &s.keys[i] == key { return Some(&s.vals[i]); }
    }
    None
}

// ---- u64 (inline, register-width) ----

fn u64_arr<const N: usize>() -> [(u64, u64); N] {
    std::array::from_fn(|i| (i as u64, (i as u64).wrapping_mul(0x9e3779b9)))
}

fn u64_soa<const N: usize>() -> Soa<u64, u64, N> {
    Soa {
        keys: std::array::from_fn(|i| i as u64),
        vals: std::array::from_fn(|i| (i as u64).wrapping_mul(0x9e3779b9)),
    }
}

fn u64_miss() -> u64 { u64::MAX } // not in 0..N

// ---- SIMD SoA (u64 only; String eq has no SIMD lane mapping) ----

fn lookup_soa_simd_u64_8(s: &Soa<u64, u64, 8>, key: u64) -> Option<u64> {
    let v = u64x8::from_array(s.keys);
    v.simd_eq(u64x8::splat(key)).first_set().map(|i| s.vals[i])
}

fn lookup_soa_simd_u64_16(s: &Soa<u64, u64, 16>, key: u64) -> Option<u64> {
    let v = u64x16::from_array(s.keys);
    v.simd_eq(u64x16::splat(key)).first_set().map(|i| s.vals[i])
}

// `any()`-first: on a miss skip the kmov+tzcnt that `first_set` always pays.
fn lookup_soa_simd_any_u64_8(s: &Soa<u64, u64, 8>, key: u64) -> Option<u64> {
    let m = u64x8::from_array(s.keys).simd_eq(u64x8::splat(key));
    if m.any() { Some(s.vals[m.first_set().unwrap()]) } else { None }
}

fn lookup_soa_simd_any_u64_16(s: &Soa<u64, u64, 16>, key: u64) -> Option<u64> {
    let m = u64x16::from_array(s.keys).simd_eq(u64x16::splat(key));
    if m.any() { Some(s.vals[m.first_set().unwrap()]) } else { None }
}

// 256-bit (u64x4): Zen4 runs ymm at full throughput vs zmm at ~half.
// Pack the per-chunk 4-bit masks into one integer, one tzcnt at the end.
fn lookup_soa_simd256_u64_8(s: &Soa<u64, u64, 8>, key: u64) -> Option<u64> {
    let k = u64x4::splat(key);
    let lo: [u64; 4] = s.keys[..4].try_into().unwrap();
    let hi: [u64; 4] = s.keys[4..].try_into().unwrap();
    let m = u64x4::from_array(lo).simd_eq(k).to_bitmask() as u8
        | ((u64x4::from_array(hi).simd_eq(k).to_bitmask() as u8) << 4);
    if m == 0 { None } else { Some(s.vals[m.trailing_zeros() as usize]) }
}

fn lookup_soa_simd256_u64_16(s: &Soa<u64, u64, 16>, key: u64) -> Option<u64> {
    let k = u64x4::splat(key);
    let mut m: u16 = 0;
    for (c, chunk) in s.keys.chunks_exact(4).enumerate() {
        let a: [u64; 4] = chunk.try_into().unwrap();
        m |= (u64x4::from_array(a).simd_eq(k).to_bitmask() as u16) << (c * 4);
    }
    if m == 0 { None } else { Some(s.vals[m.trailing_zeros() as usize]) }
}

// ---- String (out of line, heap) ----
// Same-length keys so String equality can't short-circuit on len.

fn str_arr<const N: usize>() -> [(String, u64); N] {
    std::array::from_fn(|i| (format!("key{i:05}"), i as u64))
}

fn str_soa<const N: usize>() -> Soa<String, u64, N> {
    Soa {
        keys: std::array::from_fn(|i| format!("key{i:05}")),
        vals: std::array::from_fn(|i| i as u64),
    }
}

fn str_miss() -> String { "key99999".to_string() } // same len, not present

macro_rules! bench_aos {
    ($name:ident, $n:tt, $kt:ty, $make:expr, $miss:expr) => {
        #[bench]
        fn $name(b: &mut Bencher) {
            let data: [($kt, u64); $n] = $make;
            let key = $miss;
            b.iter(|| black_box(lookup_aos(black_box(&data), black_box(&key))));
        }
    };
}

macro_rules! bench_soa {
    ($name:ident, $n:tt, $kt:ty, $make:expr, $miss:expr) => {
        #[bench]
        fn $name(b: &mut Bencher) {
            let data: Soa<$kt, u64, $n> = $make;
            let key = $miss;
            b.iter(|| black_box(lookup_soa(black_box(&data), black_box(&key))));
        }
    };
}

// u64, N = 8 / 16
bench_aos!(u64_aos_n8, 8, u64, u64_arr::<8>(), u64_miss());
bench_soa!(u64_soa_n8, 8, u64, u64_soa::<8>(), u64_miss());
bench_aos!(u64_aos_n16, 16, u64, u64_arr::<16>(), u64_miss());
bench_soa!(u64_soa_n16, 16, u64, u64_soa::<16>(), u64_miss());

// SIMD SoA, u64 N = 8 / 16
#[bench]
fn u64_soa_simd_n8(b: &mut Bencher) {
    let data = u64_soa::<8>();
    let key = u64_miss();
    b.iter(|| black_box(lookup_soa_simd_u64_8(black_box(&data), black_box(key))));
}
#[bench]
fn u64_soa_simd_n16(b: &mut Bencher) {
    let data = u64_soa::<16>();
    let key = u64_miss();
    b.iter(|| black_box(lookup_soa_simd_u64_16(black_box(&data), black_box(key))));
}

// SIMD SoA, any()-first, u64 N = 8 / 16
#[bench]
fn u64_soa_simd_any_n8(b: &mut Bencher) {
    let data = u64_soa::<8>();
    let key = u64_miss();
    b.iter(|| black_box(lookup_soa_simd_any_u64_8(black_box(&data), black_box(key))));
}
#[bench]
fn u64_soa_simd_any_n16(b: &mut Bencher) {
    let data = u64_soa::<16>();
    let key = u64_miss();
    b.iter(|| black_box(lookup_soa_simd_any_u64_16(black_box(&data), black_box(key))));
}

// SIMD SoA, 256-bit (u64x4), u64 N = 8 / 16
#[bench]
fn u64_soa_simd256_n8(b: &mut Bencher) {
    let data = u64_soa::<8>();
    let key = u64_miss();
    b.iter(|| black_box(lookup_soa_simd256_u64_8(black_box(&data), black_box(key))));
}
#[bench]
fn u64_soa_simd256_n16(b: &mut Bencher) {
    let data = u64_soa::<16>();
    let key = u64_miss();
    b.iter(|| black_box(lookup_soa_simd256_u64_16(black_box(&data), black_box(key))));
}

// String, N = 8 / 16
bench_aos!(str_aos_n8, 8, String, str_arr::<8>(), str_miss());
bench_soa!(str_soa_n8, 8, String, str_soa::<8>(), str_miss());
bench_aos!(str_aos_n16, 16, String, str_arr::<16>(), str_miss());
bench_soa!(str_soa_n16, 16, String, str_soa::<16>(), str_miss());

// ---- streaming: 1M blocks, lookup in each, accumulate the value ----
// Bottleneck shifts to memory bandwidth. Probe = last key of each block
// (present → maps to a value, but sits at the final position so the scan
// walks all N). Blocks are identical so the work per block is fixed; the
// cost being measured is touching 1M × (block bytes) of memory.

const BLOCKS: usize = 1024 * 1024;

fn u64_stream_aos<const N: usize>() -> Vec<[(u64, u64); N]> {
    let block = std::array::from_fn(|i| (i as u64, (i as u64).wrapping_mul(0x9e3779b9)));
    vec![block; BLOCKS]
}

fn u64_stream_soa<const N: usize>() -> Vec<Soa<u64, u64, N>> {
    let block = Soa {
        keys: std::array::from_fn(|i| i as u64),
        vals: std::array::from_fn(|i| (i as u64).wrapping_mul(0x9e3779b9)),
    };
    vec![block; BLOCKS]
}

macro_rules! bench_stream_aos {
    ($name:ident, $n:tt) => {
        #[bench]
        fn $name(b: &mut Bencher) {
            let stream = u64_stream_aos::<$n>();
            let key = ($n - 1) as u64; // last key: full scan, returns a value
            b.iter(|| {
                let mut acc = 0u64;
                for blk in stream.iter() {
                    if let Some(v) = lookup_aos(blk, &key) { acc ^= v; }
                }
                black_box(acc)
            });
        }
    };
}

macro_rules! bench_stream_soa {
    ($name:ident, $n:tt) => {
        #[bench]
        fn $name(b: &mut Bencher) {
            let stream = u64_stream_soa::<$n>();
            let key = ($n - 1) as u64;
            b.iter(|| {
                let mut acc = 0u64;
                for blk in stream.iter() {
                    if let Some(v) = lookup_soa(blk, &key) { acc ^= v; }
                }
                black_box(acc)
            });
        }
    };
}

macro_rules! bench_stream_simd {
    ($name:ident, $n:tt, $lookup:expr) => {
        #[bench]
        fn $name(b: &mut Bencher) {
            let stream = u64_stream_soa::<$n>();
            let key = ($n - 1) as u64;
            b.iter(|| {
                let mut acc = 0u64;
                for blk in stream.iter() {
                    if let Some(v) = $lookup(blk, key) { acc ^= v; }
                }
                black_box(acc)
            });
        }
    };
}

bench_stream_aos!(u64_stream_aos_n8, 8);
bench_stream_soa!(u64_stream_soa_n8, 8);
bench_stream_simd!(u64_stream_simd_n8, 8, lookup_soa_simd_u64_8);
bench_stream_simd!(u64_stream_simd256_n8, 8, lookup_soa_simd256_u64_8);

bench_stream_aos!(u64_stream_aos_n16, 16);
bench_stream_soa!(u64_stream_soa_n16, 16);
bench_stream_simd!(u64_stream_simd_n16, 16, lookup_soa_simd_u64_16);
bench_stream_simd!(u64_stream_simd256_n16, 16, lookup_soa_simd256_u64_16);
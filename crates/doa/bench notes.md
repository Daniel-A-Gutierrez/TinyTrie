find slot

find_slot bench results

┌────────────────────────┬────────────────────┬──────────────────────────────────────────────────┐
│         bench          │        time        │                      notes                       │
├────────────────────────┼────────────────────┼──────────────────────────────────────────────────┤
│ twophase_near_s2/s8    │ 6.6 / 7.2 ns       │ rank-1 hit, both strides                         │
├────────────────────────┼────────────────────┼──────────────────────────────────────────────────┤
│ twophase_far_s2/s8     │ 12.7 / 13.3 ns     │ rank-14 hit                                      │
├────────────────────────┼────────────────────┼──────────────────────────────────────────────────┤
│ twophase_append_s2/s8  │ 13.4 / 13.5 ns     │ ~budget/2 probes                                 │
├────────────────────────┼────────────────────┼──────────────────────────────────────────────────┤
│ twophase_prepend_s2/s8 │ 12.1 / 12.3 ns     │ ~budget/2 probes                                 │
├────────────────────────┼────────────────────┼──────────────────────────────────────────────────┤
│ twophase_miss_s2/s8    │ 16.0 / 15.0 ns     │ full budget, no hit                              │
├────────────────────────┼────────────────────┼──────────────────────────────────────────────────┤
│ stream_twophase_s8     │ 4,655 ns           │ cold-cache sweep, 256 inner iters → ~18 ns/probe │
├────────────────────────┼────────────────────┼──────────────────────────────────────────────────┤
│ scan_near/far/miss     │ 1.8–1.8k ns        │ O(len) baseline                                  │
├────────────────────────┼────────────────────┼──────────────────────────────────────────────────┤
│ lin_near/far/miss      │ 8.9 / 95 / 2.6k ns │ outward scan w/ early exit                       │
└────────────────────────┴────────────────────┴──────────────────────────────────────────────────┘

layout_bench — AoS vs SoA K->V lookup (benches/layout_bench.rs)
================================================================

Compares `[(K,V); N]` (AoS) vs `struct { keys: [K;N], vals: [V;N] }` (SoA),
plus a manual `std::simd` SoA variant. K = u64 (inline) and String (out of
line); N = 8 and 16. CPU: Ryzen 9 8945HS (Zen4, AVX-512 incl. f/vl/bw/dq).
Run: `cargo bench -p doa --bench layout_bench`.

Single-block probe (miss → full N-scan, isolates per-block compute cost):

┌──────────────────────┬───────────────┬───────────────┐
│ variant               │ N=8           │ N=16          │
├──────────────────────┼───────────────┼───────────────┤
│ u64 AoS (scalar)      │ 1.47 ns       │ 2.25 ns       │
│ u64 SoA (scalar)      │ 1.48 ns       │ 2.41 ns       │
│ u64 SoA SIMD 512-bit  │ 6.99 ns       │ 5.46 ns       │
│ u64 SoA SIMD any-first│ 6.99 ns       │ 5.45 ns       │
│ u64 SoA SIMD 256-bit  │ 5.81 ns       │ 5.90 ns       │
│ str AoS / SoA         │ 13.5 / 14.5 ns│ 27 / 28 ns    │
└──────────────────────┴───────────────┴───────────────┘

Streaming probe (1M blocks, hit at last position → full scan + value):

┌──────────────────────┬───────────────────┬───────────────────┐
│ variant               │ N=8 (128 MB)      │ N=16 (256 MB)      │
├──────────────────────┼───────────────────┼───────────────────┤
│ u64 AoS (scalar)      │ 4.34 ms (~30 GB/s)│ 8.40 ms (~30 GB/s) │
│ u64 SoA (scalar)      │ 4.31 ms           │ 11.79 ms (~21 GB/s)│
│ u64 SoA SIMD 512-bit  │ 4.11 ms           │ 10.97 ms           │
│ u64 SoA SIMD 256-bit  │ 4.35 ms           │ 12.48 ms           │
└──────────────────────┴───────────────────┴───────────────────┘

Findings
--------

Single-block: scalar beats SIMD 3–5×. The scalar loop is NOT autovectorized
— linear search with early-exit branches the predictor nails on a miss
(~1 cycle/compare). SIMD's fixed cost (splat + cmp + movemask + tzcnt)
exceeds that whole sequence at N=8/16; asm confirmed the hot path is already
minimal. `any()`-first is a no-op (identical asm). 256-bit helps N=8 ~17%
(Zen4 runs zmm at ~half throughput) but hurts N=16 (4 movemasks + OR packing);
still ~4× slower than scalar. String: AoS ≈ SoA, pointer-chase bound; SIMD
doesn't apply (String eq is len + memcmp, no lane mapping).

Streaming: flips to AoS wins, SoA loses.
1. SIMD overhead vanishes — the ~5 ns/block gap is ~5 ms over 1M blocks and
   it's gone; lookup compute runs while stalled on RAM. SIMD is pointless
   when memory-bound.
2. Per-block SoA loses the bandwidth battle: every block is a hit, so SoA
   reads keys AND jumps to vals — prefetcher grabs the whole struct either
   way, same bytes as AoS. Worse, the SoA val load is *dependent* on the key
   match (issues after the scan resolves) so it can't prefetch as early; AoS
   reads the val speculatively in the sequential tuple scan, zero extra
   latency. At N=16 the val is 2 lines from the key scan → stalls →
   30→21 GB/s; at N=8 it's the next line → SoA ≈ AoS.

Takeaways for doa (scanning leaf blocks for a key you'll retrieve):
- AoS wins when hits need their value — the value rides along free; SoA
  makes it a dependent load.
- SoA only wins when you scan keys WITHOUT touching vals (miss probe, or
  two-pass filter-then-fetch) — that's when it touches half the bytes.
- SIMD helps neither regime at N=8/16: single-block branch-prediction beats
  it; streaming is memory-bound so its compute edge is moot.

Untried (would show SoA's real win): miss probe over GLOBAL SoA — all 1M
blocks' keys in one contiguous array, vals in a separate array — vs global
AoS. A miss touches only the keys array (half the bytes) → SoA ~2× faster.
Per-block SoA here interleaves keys/vals at block granularity, denying that.


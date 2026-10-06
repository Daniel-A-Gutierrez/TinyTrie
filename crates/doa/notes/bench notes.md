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

# Address translation benchmark
all zero             1.683 ns/iter
all nonzero          1.660 ns/iter

The CPU does not skip runtime no-ops. Every config — including the one where offset=shift=rotation=0, so v2p is add 0 → shl 0 → ror 0 (a pure identity) — runs at the same ~1.66 ns/iter as all nonzero. The zero-parameter loop is not faster.

Setup: arr[256] perm, x=v2p(arr[x]) dependent chain, iters=2^28, params
black_boxed so the compiler can't fold no-ops (forces real ror/shl/add r,0).

The uniform v2p math `(v+o)<<s ror r` is wrong-shaped: when o/s/r are 0 the
CPU still issues and executes the no-op ALU ops (add 0, shl 0, ror 0) — they
sit in the critical `x` dependency chain and cost the same as real work. The
CPU does not detect/elide runtime no-ops. They're only free if the compiler
sees the zero at compile time.

Three translation shapes, ns/iter:

  straight (uniform math)   all-zero 1.625   all-nonzero 1.628
  branchy (guard each op)    all-zero 1.434   all-nonzero 1.626   50/50 2.037
  fn-ptr (specialized body)  all-zero 1.241   all-nonzero 1.686

- branchy: guard each op with `if param != 0`. The cmp/jne guards are on the
  loop-invariant params, NOT on v, so they don't extend the v chain — v flows
  through, the branches resolve in parallel. ~12% faster on all-zero, free on
  all-nonzero (predicted-taken). BUT a 50/50 alternating param pattern
  mispredicts every iter (+0.40ns, the floor) — risky if params churn.
- fn-ptr: block stores `v2p: fn(...)`, set_params picks a pre-baked body that
  skips its zero-param ops (f_id = `x`, f_o = `x+o`, etc, straight-line, no
  per-iter branch). Dispatch happens ONCE at set_params, not per-iteration, so
  the 50/50 mispredict floor disappears — the call target is constant for as
  long as the block keeps its params. all-zero body is `return x` → ~24%
  faster than uniform, beats branchy too. all-nonzero pays only the indirect
  call/ret overhead (~+0.06ns, <1 cycle on the x chain) on top of real work.

Best of both worlds: fn-ptr specialization. No per-iter branch, no mispredict
risk, shortest possible body when params are zero, ~free when they aren't.

Relevance to doa: the addressor should store a v2p fn ptr and re-point it in
set_params rather than running the uniform `(v+o)<<s ror r` math every lookup.
Append/prepend blocks (shift=0) get the ~24% win automatically; a block that
graduates / changes strategy pays one set_params + a ~0.06ns indirect call.
For blocks whose strategy is statically known (const-generic), inline still
beats even this — fn-ptr is the shape for the *adaptive* tier; const-generic
inlining is the shape for fixed-strategy blocks.

spread (i -> 2*i, len doubles)
============================

spread_bench.rs. VecDeque<Option<usize>>, m=1<<14 (16384, pow2 — phase-2 halving
needs it). Spread-only = raw minus the matching make_only baseline (contig 3.5us,
wrapped 13.4us). Approaches:

- R pushnone: push_back(None) m times + 1-pass reverse move i->2i. ~3m writes.
  (= current production DequeStore::spread: resize_with + reverse.)
- U interleaved_halving: phase1 take upper half [mid,m), push each then a None
  -> upper half lands in final [m,2m) (evens=value, odds=None, NO wasted
  even-slot None-init); phase2 in-place halving of lower half over [0,m).
  ~2.5m writes. SAFE on VecDeque (the push_back trick replaces spare_capacity_mut).
- U' interleaved_reverse: same phase1, phase2 = single reverse-move (same writes
  as U's phase2, linear sweep, no recursion).
- U'+slice: phase1, then make_contiguous() and run phase2 on the returned &mut[T].

  contig (spread-only)        wrapped (spread-only)
  R  pushnone            30.3us   --
  U  interleaved_halving 25.3us   --
  U' interleaved_reverse 27.1us  22.6us
  U'+slice              19.4us  25.8us  (slower on wrapped!)

Findings:
- Interleaved beats pushnone: U 25.3 vs R 30.3 (~16%). The 2.5m-vs-3m write saving
  is real and safe on VecDeque. push_back-interleaving places the upper half into
  final position without the wasted even-slot None-init — same saving
  spare_capacity_mut gives on Vec, but via the safe push API. (Key insight: the
  new tail is built one slot at a time, so every slot is written exactly once
  with its final value; resize_with instead pre-inits the whole tail then
  overwrites half of it.)
- Phase2 halving vs reverse-move: a wash (25.3 vs 27.1, within +/-1.7us noise).
  Pick reverse-move for less code.
- make_contiguous + slice indexing is the biggest effect, but conditional:
  * contig deque: huge win. U'+slice 19.4 vs U' 27.1 (~28%). make_contiguous is a
    FREE no-op here; the win is entirely slice indexing skipping the deque's
    per-access (head+i)%cap (~0.5ns/access x 16k = the whole 7.7us gap). Deque
    indexing is expensive; slice indexing isn't.
  * wrapped deque: net loss. U'+slice 25.8 vs U' 22.6 (~15% slower). The O(n)
    linearization costs more than slice-indexing saves.
  => rule: branch on contiguity. contig (as_slices().1.is_empty()) -> use
     as_mut_slices().0 for cheap slice-indexed phase2 (free). wrapped -> eat the
     deque-index cost, do NOT make_contiguous.

Caveats (from audit): m=16384 is far bigger than real i8 blocks (cap 8-256) and
the input is dense all-Some, not sparse (real blocks are sparse w/ None gaps on
the AP grid). Direction (interleaved + slice wins) is robust; the 36% magnitude
will differ at real sizes/patterns. Not yet re-run at m=256 / sparse.

Production impl written into DequeStore::spread: interleaved phase1 + contig-
branched phase2 (slice when contig, deque-index when wrapped).

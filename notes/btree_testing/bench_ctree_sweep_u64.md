# CTree Parameter Sweep: u64 (fixed-width, NoPreview)

**Key mode**: random u64  
**Baseline**: `N=4, NoPreview` (current default)  
**N values tested**: 2, 4, 8, 12, 16  
**P**: NoPreview (fixed-width keys use full 64-bit SIMD comparison, no preview needed)

## Raw Results (CTreeFixed, 1M keys)

| Config | Insert | Lookup | Fwd Iter | Rev Iter | Memory |
|--------|--------|--------|----------|----------|--------|
| N=2  | 971.9K | 1.79M | 373.91M | 238.29M | 62.6 |
| **N=4** | **1.72M** | **3.84M** | **472.07M** | **288.38M** | **37.3** |
| N=8  | 2.75M | 5.04M | 656.48M | 362.85M | 27.3 |
| N=12 | 3.58M | 4.99M | 677.83M | 364.94M | 24.3 |
| N=16 | 3.70M | 4.93M | 733.71M | 494.34M | 22.8 |

## CTreeFixedOpt Results (after optimize, 1M keys)

| Config | Insert | Lookup | Fwd Iter | Rev Iter |
|--------|--------|--------|----------|----------|
| N=2  | 992.7K | 1.83M | 379.06M | 234.96M |
| N=4  | 1.80M | 3.85M | 471.32M | 289.27M |
| N=8  | 2.76M | 5.09M | 659.18M | 387.53M |
| N=12 | 3.70M | 4.92M | 692.28M | 368.16M |
| N=16 | 3.71M | 4.87M | 826.04M | 503.03M |

## vs Baseline (N=4)

| Config | Insert | Lookup | Fwd Iter | Rev Iter | Memory |
|--------|--------|--------|----------|----------|--------|
| N=2  | 0.56x | 0.47x | 0.79x | 0.83x | 1.68x |
| N=4  | 1.00x | 1.00x | 1.00x | 1.00x | 1.00x |
| N=8  | 1.60x | 1.31x | 1.39x | 1.26x | 0.73x |
| N=12 | 2.08x | 1.30x | 1.44x | 1.26x | 0.65x |
| N=16 | 2.15x | 1.28x | 1.55x | 1.71x | 0.61x |

## Key Observations (u64)

1. **N=8 is the sweet spot for u64**: best lookup (5.04M), solid insert (2.75M), good fwd (656M), best overall balance. Lookup actually *decreases* for N=12 and N=16.

2. **N=16 wins insert and fwd iteration**: 3.70M insert (+115%), 733.71M fwd iter (+55%). After optimize, fwd hits 826M.

3. **Lookup plateaus at N=8**: 5.04M for N=8 vs 4.93M for N=16 — larger nodes don't help SIMD find_position on fixed-width keys since it already scans the full key.

4. **Memory efficiency improves steadily**: 37.3 → 22.8 bytes/key (39% reduction at N=16).

5. **No preview type to tune** — the u64 path uses `find_position` SIMD directly on stored keys, so fanout is the only knob.

6. **Composite winner**: N=8 offers the best balance for u64 workloads. N=16 is better if fwd iteration dominates.
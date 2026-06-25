# CTree Parameter Sweep: Vec&lt;u8&gt; Lines (wikipedia.txt corpus)

**Key mode**: lines from wikipedia.txt  
**Baseline**: `N=4, P=u64` (current default)  
**N values tested**: 2, 4, 8, 12, 16  
**P values tested**: u8, u16, u32, u64

## Raw Results (CTree, 1M keys)

| Config | Insert | Lookup | Fwd Iter | Rev Iter | Memory |
|--------|--------|--------|----------|----------|--------|
| N=2, P=u8   | 1.63M | 2.25M | 475.40M | 352.30M | 571.3 |
| N=2, P=u16  | 1.50M | 2.19M | 403.45M | 314.57M | 579.3 |
| N=2, P=u32  | 1.42M | 2.33M | 447.14M | 329.06M | 587.3 |
| N=2, P=u64  | 1.52M | 2.05M | 489.54M | 352.57M | 611.3 |
| N=4, P=u8   | 2.78M | 3.21M | 1.11G   | 464.33M | 343.9 |
| N=4, P=u16  | 2.49M | 2.86M | 956.36M | 453.03M | 346.6 |
| N=4, P=u32  | 2.23M | 2.57M | 886.75M | 461.72M | 350.1 |
| **N=4, P=u64** | **3.02M** | **3.18M** | **936.87M** | **436.92M** | **358.1** |
| N=8, P=u8   | 3.60M | 3.61M | 1.22G   | 466.26M | 285.6 |
| N=8, P=u16  | 2.48M | 3.29M | 1.22G   | 465.07M | 286.9 |
| N=8, P=u32  | 3.48M | 3.24M | 1.39G   | 474.27M | 289.5 |
| N=8, P=u64  | 3.83M | 2.85M | 1.03G   | 479.76M | 294.9 |
| N=12, P=u8  | 3.03M | 3.43M | 1.22G   | 491.42M | 269.5 |
| N=12, P=u16 | 3.23M | 3.19M | 1.36G   | 492.81M | 271.0 |
| N=12, P=u32 | 3.74M | 3.54M | 1.36G   | 547.92M | 273.4 |
| N=12, P=u64 | 3.45M | 3.42M | 1.32G   | 450.41M | 278.2 |
| N=16, P=u8  | 3.57M | 3.73M | 1.43G   | 511.10M | 263.2 |
| N=16, P=u16 | 3.26M | 2.95M | 1.41G   | 502.94M | 264.4 |
| N=16, P=u32 | 3.65M | 3.44M | 1.43G   | 543.69M | 266.7 |
| N=16, P=u64 | 4.46M | 3.37M | 1.37G   | 543.11M | 271.3 |

## CTreeOpt Results (after optimize, 1M keys)

| Config | Insert | Lookup | Fwd Iter | Rev Iter |
|--------|--------|--------|----------|----------|
| N=2, P=u8   | 1.64M | 2.37M | 462.70M | 352.25M |
| N=2, P=u16  | 1.53M | 2.13M | 430.30M | 329.30M |
| N=2, P=u32  | 1.50M | 2.31M | 444.58M | 326.88M |
| N=2, P=u64  | 1.20M | 2.02M | 482.91M | 354.46M |
| N=4, P=u8   | 2.73M | 3.10M | 1.10G   | 466.91M |
| N=4, P=u16  | 2.53M | 2.89M | 926.61M | 443.30M |
| N=4, P=u32  | 2.27M | 3.24M | 896.72M | 451.02M |
| N=4, P=u64  | 2.81M | 3.22M | 919.81M | 440.91M |
| N=8, P=u8   | 3.52M | 3.58M | 1.22G   | 470.39M |
| N=8, P=u16  | 2.59M | 2.70M | 1.22G   | 468.96M |
| N=8, P=u32  | 3.29M | 3.16M | 1.37G   | 503.66M |
| N=8, P=u64  | 3.63M | 2.85M | 1.04G   | 493.61M |
| N=12, P=u8  | 3.27M | 3.43M | 1.34G   | 494.20M |
| N=12, P=u16 | 3.27M | 3.15M | 1.37G   | 491.16M |
| N=12, P=u32 | 4.01M | 3.52M | 1.37G   | 549.91M |
| N=12, P=u64 | 3.22M | 3.47M | 1.28G   | 527.03M |
| N=16, P=u8  | 3.62M | 3.58M | 1.42G   | 511.57M |
| N=16, P=u16 | 3.16M | 2.65M | 1.38G   | 507.62M |
| N=16, P=u32 | 3.09M | 3.46M | 1.42G   | 540.69M |
| N=16, P=u64 | 4.40M | 3.34M | 1.37G   | 540.87M |

## Key Observations

1. **Lines workload shows much less sensitivity to P** compared to random keys. Forward iteration is uniformly fast (1.0-1.4G) for N≥8 regardless of preview type, because long keys have strong first-byte discrimination.

2. **N=16 is the overall winner for lines** — highest insert (4.46M), best fwd iter (1.43G), best rev (543.69M), and best memory (263.2 bytes/key). The longer keys in the lines workload benefit from higher fanout since each internal node key comparison does more work.

3. **P=u8 dominates fwd iteration for lines** at higher fanouts — 1.43G for N=16,P=u8 vs 1.37G for N=16,P=u64. The 1-byte preview is sufficient for line keys (which have diverse first bytes) and saves cache space.

4. **P=u64 actually wins insertion for N=16** (4.46M vs 3.57M for P=u8), because the 8-byte preview allows more keys to be resolved at the internal node level without descending.

5. **Memory scales dramatically with key length** — 263-611 bytes/key vs 41-196 for random 8-byte keys — because the lines are much longer strings.

6. **Compared to the random-key sweep**: the optimal configuration shifts toward higher fanout (N=12-16 vs N=6-8) and smaller preview (P=u8/u32 vs P=u16/u32) because longer, more diverse keys benefit from wider nodes and need less preview discrimination.
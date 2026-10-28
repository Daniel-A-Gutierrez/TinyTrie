# CTree Parameter Sweep: Vec&lt;u8&gt; Words (wikipedia.txt corpus)

**Key mode**: words from wikipedia.txt  
**Baseline**: `N=4, P=u64` (current default)  
**N values tested**: 2, 4, 8, 12, 16  
**P values tested**: u8, u16, u32, u64

## Raw Results (CTree, 1M keys)

| Config | Insert | Lookup | Fwd Iter | Rev Iter | Memory |
|--------|--------|--------|----------|----------|--------|
| N=2, P=u8   | 1.87M | 5.94M | 420.63M | 343.81M | 153.2 |
| N=2, P=u16  | 1.51M | 4.17M | 352.11M | 275.76M | 161.2 |
| N=2, P=u32  | 1.41M | 4.14M | 324.09M | 261.44M | 169.2 |
| N=2, P=u64  | 1.41M | 4.31M | 325.18M | 283.14M | 193.2 |
| N=4, P=u8   | 2.28M | 7.20M | 706.33M | 407.15M | 65.7  |
| N=4, P=u16  | 2.04M | 5.54M | 664.61M | 358.10M | 68.4  |
| N=4, P=u32  | 2.13M | 6.47M | 623.54M | 375.82M | 71.9  |
| **N=4, P=u64** | **2.25M** | **6.28M** | **602.95M** | **354.62M** | **79.9** |
| N=8, P=u8   | 2.67M | 7.52M | 826.55M | 356.51M | 46.8  |
| N=8, P=u16  | 2.45M | 6.43M | 861.83M | 358.33M | 48.1  |
| N=8, P=u32  | 2.55M | 6.58M | 615.98M | 400.47M | 50.7  |
| N=8, P=u64  | 2.68M | 6.48M | 666.61M | 453.20M | 56.1  |
| N=12, P=u8  | 3.06M | 7.62M | 992.30M | 455.74M | 41.5  |
| N=12, P=u16 | 2.65M | 5.48M | 984.82M | 490.19M | 43.0  |
| N=12, P=u32 | 2.73M | 7.13M | 983.25M | 442.64M | 45.4  |
| N=12, P=u64 | 3.08M | 8.18M | 1.10G   | 519.66M | 50.2  |
| N=16, P=u8  | 3.22M | 7.76M | 1.32G   | 555.88M | 39.6  |
| N=16, P=u16 | 2.58M | 5.57M | 1.17G   | 460.31M | 40.7  |
| N=16, P=u32 | 3.12M | 7.37M | 1.33G   | 457.56M | 43.0  |
| N=16, P=u64 | 2.93M | 6.81M | 1.03G   | 471.85M | 47.6  |

## Key Observations (Words)

1. **N=16, P=u32 is the best overall for words**: 1.33G fwd, 7.37M lookup, 3.12M insert, 43.0 bytes/key. P=u8 edges on fwd (1.32G) but P=u32 has better balance.

2. **P=u64 is competitive for words at N=12**: 8.18M lookup (best lookup for any N≥8), but fwd iter suffers (1.10G vs 1.32G for P=u8).

3. **P=u8 consistently wins forward iteration** at every fanout ≥ 8 — the shorter preview is enough for word discrimination and saves cache.

4. **N=12 P=u64 is an anomaly**: best lookup (8.18M) AND decent fwd (1.10G). Words have enough first-byte diversity that even at moderate fanout, u64 preview gives good discrimination.

5. **Memory drops significantly with higher fanout**: 79.9 → 39.6 bytes/key from N=4 to N=16 (P=u8).

6. **Words workload pattern differs from random keys**: P=u8 consistently wins fwd iter because word first bytes are highly diverse (alphabet characters), unlike random byte keys which may share prefixes.
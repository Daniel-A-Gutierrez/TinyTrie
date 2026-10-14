# Bench notes — sortedblock insert

`benches/sorted_insert.rs` (harness=false): 10k sequential u64 keys,
xorshift-shuffled, inserted into a fresh `BlockSet<u64, u16>`. 10 runs,
best/avg ns per insert; len + strict order gated before a run counts.
2026-10-12, dev machine.

## optimization ladder (all on the same bench)

| change | best | avg |
|---|---|---|
| fresh-alloc grow, bit-at-a-time hole scan, per-element slide | 385.7 | 431.5 |
| in-place grow (descending 2·pos pass + word-wise mask spread) | 379.2 | 386.3 |
| word-wise hole scan (`Mask::nearest_clear`/`next_clear`/`prev_clear`) | ~260 | ~267 |
| slide = one `ptr::copy` + O(1) mask (run fully live ⇒ single set + clear) | ~123 | ~130 |
| Deltas compact event (`NoneSlide{from,to}` enum, no per-pair Vec pushes) | **~86** | **~90** |

- grow was amortized noise (12 turns over 10k inserts); the scan and the
  slide were the real targets. word-wise scans: −32%. memmove slide: −2×.
  compact delta event: −30% more.
- recording deltas as per-element `(Pos,Pos)` pairs cost ~149 cycles/insert
  — 2× the memmove it described. one contiguous run shift = one 16-byte
  enum write; `Deltas::iter` re-expands to the identical swap cascade
  (consumer contract unchanged).

## phase breakdown (rdtsc scaffolding, since removed)

cycles/insert at final state, ~3.5 GHz effective:

- bsearch (`position`): ~231 — the remaining hotspot, ~⅔ of insert
- hole find (`nearest_clear`): ~38
- slide (memmove + 2 mask bits + event write): ~41
- mint (place + mask + p2a): ~29
- grow (amortized): ~18

method note: `Instant` pairs (~25ns) swamp a ~120ns insert; rdtsc pairs are
~5ns. non-serialized rdtsc under OOO hides its own overhead (empty-phase
counter read 0), so phases are mildly optimistic but mutually consistent —
they sum to the instrumented wall.

## bsearch verdict

left alone deliberately. at 10k keys/16k slots the key array is ~128KB;
each of ~14 probes hits a fresh cache line, mostly outside L1 — the cost is
memory latency, not instructions. the probe loop is already minimal
(word-wise hole skip + one compare). eytzinger/branchless tricks only pay
when the array is cache-resident. lookup gets won structurally instead:
bounded block caps in the arena keep per-block keys L1/L2-resident, and the
separator routing pays the same log-n misses any B+ tree would.

## biases → unified ladder (2026-10-12/13, same bench)

The bench now runs three workloads (append = ascending keys, prepend =
descending, random = shuffled) × unbiased/matched-bias columns, with 32-byte
items (index-sized payload — element moves pay realistic byte costs, and the
pair-cascade cost a holder pays is proportional to run lengths). ns/insert,
best-of-10:

random workload, ladder of measured steps (32B):

| space policy | best |
|---|---|
| lazy turn (fire at mask-full) | 99.3 |
| preemptive turn at ≥80% full | 88.7 |
| unified ladder (probe→back-open→dense/local turn→fluke) | 87.0 |

sequential workloads: 8.3 both columns after the edge rule (`spot == cap`
pushes back regardless of strategy) + the cadence fix (the hole goes BELOW
k — sequential inserts can't consume it; before the fix the trailing hole
was eaten by the next append, the block stayed dense, and every push_back
paid a full-mask `prev_clear` + a doubled push: 64 ns/append). Before the
edge rule, Spread-under-sequential ran 615 (32B) / 225 (8B) — once the
interleaved tail holes deplete, every append slides from a distant front
hole: O(n) slide + ~n-pair cascade per insert.

findings:

- element size amplifies the preemption win: −6% at 8B → −11% at 32B. turn
  amortization is a wash by design (turns at 80% move 20% fewer elements 20%
  sooner, ~2 moves/insert either way) — the win is entirely bounded slides and
  short cascades. turns are the only zero-churn growth (holders see an empty
  deltas), so the holder-side win exceeds what this bench shows (it clears
  deltas without applying them).
- the wrong-direction column (descending keys into an Asc block: 692) is
  irreducible: every insert must move the prefix, and slide-regimes and
  turn-regimes amortize to the same order. its fix is the right direction
  (Desc: 64), not the ladder.
- the local-density turn gate (nearest hole > 4·budget AND len ≥ ⅔ cap — a
  spread never leaves the block below ⅓) is dormant for random (max gap ~26
  at 67% density, under the 4·budget=64 trip), costs ~10% on the
  wrong-direction column, and guards sparse blocks from cap-doubling on hot
  clusters. below the ⅔ floor the wide slide stands (accepted: space over
  slides) — clustered streams under the floor still slide wide.

## 1:1 vs std BTreeSet (32B items, 10k keys, best-of-10)

| workload | insert BS/BTS | lookup BS/BTS |
|---|---|---|
| append | 10.4 / 29.3 | 64.4 / 41.0 |
| prepend | 10.3 / 23.5 | 69.0 / 41.6 |
| random | 90.2 / 50.4 | 68.4 / 41.0 |

sequential insert: BlockSet ~2.5× faster (push_back + amortized Vec vs node
alloc + rebalance). random insert: BTreeSet 1.8× faster (within-node
memcpys, no bookkeeping, vs bsearch + probe + slide + event + turns).
lookup: BTreeSet 1.6× faster — the documented bsearch cache story (~14
probes over a 320KB array); the design answer is structural (bounded block
caps + separator routing in the arena tier), and this comparison is one
10k block vs a whole tree — a leaf against a B+ set. BTS returns no stable
names, no zero-churn growth, no flat serializable array.

## related (earlier, separate bench)

`benches/sorted_find_slot.rs`: occupancy-mask find vs `VecStore` slot scan
— mask 1.28× (budget 16) / 1.45× (budget 320) at 80% occupancy, 320 slots.
density sweep at 90–97% still unwritten.
## fetch-gather + reversal tier (2026-10-13, session 5; 32B items, 10k keys, best-of-10)

same harness, same machine-day as the ladder rework. the ladder's cadence/
budget rungs died for `gather_slots` (fetch = 1 hole per FREE_STRIDE=8 of
distance to the nearest hole, one-pass segment compaction) + a preemptive
7/8 turn (insert > stride from the edge) + `Delta::Gather(Vec<NoneSlide>)`
chained cascade; then the reversal tier (translator fn-ptr per direction,
`reverse()` mirror, pos_sum trigger at len/8 past a 32 warmup).

| workload | old ladder | chained gather | one-pass | + 7/8 turn | + reversal |
|---|---|---|---|---|---|
| append | 10.5 | — | — | 7.6 | 8.5 |
| prepend | 10.3* | — | — | — | 74 → **8.5** |
| random | 93.5 | 1704 | 153 | 98 → 93.5 | 99 |

*prepend's old 10.3 rode the Desc bias (tail pushes); the comparable
wrong-direction number was 692–705.

- chained per-slide application was quadratic in fetch distance (random
  1704): each of the w slides re-copied the whole run. the one-pass
  segment compaction (each member crosses once, far segments first)
  fixed the mechanism, not the policy.
- the 7/8 preemptive turn is the devlog line-199 rule: without it density
  runs to mask-full and fetches blow past the stride late in each fill
  cycle (random 153 → 98). 80% vs 7/8 measured identical — the line is
  not sensitive there.
- `want = 1` fast path (hole within a stride, no Vec) recovered ~4ns on
  random (every mid insert was heap-allocating the hole list); banking
  itself costs ~2ns there — a workload that can't use the bank.
- the reversal trigger at len/4 flipped a pure uniform stream 4× (first
  at len 9 — the warmup window is noise, and each flip's era-vote echoed
  larger). each flip was expensive only because flipped blocks couldn't
  turn (offset ≠ 0 refusal, since deleted); random degraded to wide
  tail-hole slides (822). at len/8 + warmup 32: random flips 0, descending
  flips exactly 1, and post-flip turning is healthy (cap 10039 on 10k
  descending, not the degenerate push-per-insert growth).
- fn-ptr p2a/a2p + the trigger check cost ~1ns on the append hot path
  (7.6 → 8.5, part noise) — the BTB-predicted indirect call, as in
  treeblock's translator.
- unresolved (report-only): the 7/8 turn fires before checking whether
  the spot is already open — delete-max + reinsert-max pays a full turn
  for a free slot. a nearest-hole-within-stride guard is the candidate;
  unbenched.

## sorted-vec crate + runs workload (2026-10-13; `benches/sorted_insert.rs`)

`sorted_insert.rs` now runs BlockSet vs `SortedSet` (sorted-vec crate —
bsearch + `Vec::insert` memmove, deref binary_search for lookup) vs BTreeSet
over four workloads, plus a new runs stream: 100 chunks of 100 sequential
keys, half the chunks reversed to descending runs, chunk order shuffled.
10k 32B items, best-of-10, same machine-day:

insert (ns):

| workload | BlockSet | SortedSet | BTreeSet |
|---|---|---|---|
| append | 8.1 | 11.9 | 30.2 |
| prepend | 8.3 | 1092.5 | 23.3 |
| random | 96.6 | 569.2 | 53.8 |
| runs | 85.1 | 540.2 | 28.7 |

lookup (ns): BlockSet 68–71 all workloads, SortedSet 13.0–13.1, BTreeSet
42–43.

- SortedSet insert is the unbounded-memmove story everywhere except append:
  every mid-vector insert shifts the entire suffix (avg ~5k·32B, O(n²)
  over the stream) — 1093 prepend / 569 random / 540 runs. append rides
  the O(1) tail push but still loses to BlockSet (Vec doubling copies the
  payload each turn; the block's rotation turn moves names only).
- runs is the workload the block was built for: slides are
  fetch-distance-bounded (each chunk is local, holes gather beside it),
  the reversal tier absorbs the descending half. BTreeSet still wins —
  runs land within-node (a 30-element node memcpy beats bsearch + slide +
  event); tree-friendly by construction.
- BlockSet loses insert to BTreeSet on random (96.6 vs 53.8) and runs
  (85.1 vs 28.7): the dense block pays bsearch + probe + slide + event
  per insert; the tree pays a node memcpy amortized across 30 elements.
- lookup: the flat Vec is 5× faster than the block's masked probe (13.0 vs
  68–71) — contiguous, no hole skips, no fn-ptr translate. the price of
  mid-insert in a single 10k block; the structural answer (bounded block
  caps + separator routing) is the arena tier, unchanged.

### seq-read (iter first→last, key checksum; ns/elem)

| workload | BlockSet | SortedSet | BTreeSet |
|---|---|---|---|
| append | 2.01 | 0.20 | 0.90 |
| prepend | 2.80 | 0.21 | 0.82 |
| random | 2.02 | 0.21 | 0.91 |
| runs | 2.02 | 0.21 | 0.87 |

- the pass is invariant across the RUNS loop — a hoisting hazard (LLVM
  lifting the whole 10k scan out of the timing). guarded: a per-run seed
  xor-then-adds into each element (un-factorable). numbers identical
  unguarded, so no hoisting was occurring; the Vec's 0.20 is real —
  320KB is L2-resident and only the 8B key of each 32B item is touched.
- BlockSet 2.0 = the per-element mask scan (`next_set` from cursor) +
  fn-ptr p2a + deref vs the Vec's plain slice pointer bump. prepend's
  2.8 is the reversed path (`prev_set` + the negated translator).
  report-only: `iter` scans bit-by-bit — a run-based `iter_set` (yield
  occupied runs, walk within) would cut the per-element scan to a
  word-load amortized over the run; unmeasured.

### seqread iter rework (same day, same machine)

`BlockSet::iter` rebuilt on the differential: direction picked once (enum
`Iter::Asc`/`Iter::Desc` — no per-next rev check, no `from_fn` state
machine), bodies word-stepping — one loaded mask word, clear-lowest (tzcnt)
per element Asc / clear-highest (lzcnt) Desc, empty words skipped whole
(`Mask::iter_set`/`iter_set_rev`, concrete `SetBits`/`SetBitsRev`). the
per-element `next_set`/`prev_set` scans are gone; `Drop`'s `iter_set` ride
is the same engine. ns/elem best:

| workload | before | after |
|---|---|---|
| append | 2.01 | 1.62 |
| prepend | 2.80 | **1.41** |
| random | 2.02 | 1.64 |
| runs | 2.02 | 1.62 |

Desc halved — `prev_set`'s per-element keep-mask loop was the cost; the
descending word walk is now cheaper than the ascending one (bit-trick +
downward prefetch; ±0.2 noise between them). remaining gap to the flat Vec
(0.21): the per-element `(wi << 6) + tz` position rebuild + the enum
dispatch — both structural to a masked array.

## 1M scale (2026-10-14; same bench, size via CLI arg)

`sorted_insert.rs` now takes a size arg; every structure × workload cell is
capped at 3s — a slow cell (SortedSet's O(n²) inserts, the ~330ns lookups)
finishes one partial rep, reported over its completed prefix and marked
`[partial done]` (prefix figures understate the asymptote: the dense tail
never runs). BlockSet addr widened u16 → u32 (u16's name space caps at 64k).
SortedSet/BTreeSet lookup+scan setups build from a sorted copy — content is
stream-order-independent and the workload-order build is the O(n²) being
excluded. 10k baselines unchanged (u32 shift: noise).

| ns, 1M | insert | lookup | seqread |
|---|---|---|---|
| append BS/SS/BTS | 12.8 / 22.6 / 53.2 | 328 / 256 / 299 | 1.9 / 0.6 / 8.8 |
| prepend BS/SS/BTS | 11.4 / 20700* / 15.6 | 334 / 263 / 301 | 1.5 / 0.5 / 8.8 |
| random BS/SS/BTS | 487* / 14753* / 213.8 | 357 / 256 / 299 | 2.2 / 0.6 / 8.6 |
| runs BS/SS/BTS | 131.9 / 13983* / 37.2 | 421 / 261 / 301 | 2.2 / 0.6 / 8.6 |

*partial prefix, marked above; SortedSet quadratics covered ~150–220k of 1M.

- the cache cliff arrived as documented: BlockSet random insert 96 → 487
  (bsearch probes walk 32–42MB, every probe an L3 miss), lookup 68 → 330–420.
  BTreeSet degrades gently (top levels stay cached: lookup 42 → 299) and now
  WINS lookup against the block — the arena tier's separator routing is the
  answer, unchanged.
- runs is the block's scale-proof workload: 132 ns at 1M (85 at 10k —
  locality keeps slides fetch-bounded and the bsearch landing zone warm;
  random pays the full miss cascade). 100× faster than SortedSet's prefix.
- seqread is scale-invariant for the block (1.5–2.2 at both sizes) — the
  masked scan streams at DRAM speed. SortedSet's flat-Vec edge shrank
  6× (0.2 → 0.6, L2 no longer holds it); BTreeSet's walk went 0.9 → 8.8
  (scattered nodes × 32MB footprint).
- append/prepend barely moved (8.5 → 12.8/11.4): tail pushes and the
  reversal tier are cache-slope-immune.

## batched lookup — the model was wrong about spread keys (2026-10-14)

`lookup_batch<const B>`: batch sorted to pos order (Desc: batch reverses),
distinct-run dedup, median-partition descent — each search inherits the
bound its parent found. `benches/sorted_lookup_batch.rs` (random-built
block, probes = shuffled present keys, gated against per-key `find`):

| ns/lookup | single | B=4 | B=16 | B=64 | B=64 adjacent |
|---|---|---|---|---|---|
| 10k DFS-recursive | 72 | 82 | 78 | 69.5 | — |
| 10k frontier | 74 | 121 | 108 | 95 | 23.8 |
| 1M DFS-recursive | ~396 | — | — | 663 | 31.3 |
| 1M frontier | 322 | 740 | 513 | 358 | 51.4 |

- **corrected model**: the descent saves ~log2(B) − 2 probes per element,
  not log2(B) — an element at recursion depth d starts its search at
  range n/2^d, and the depth distribution (2^d elements at depth d) puts
  the average saving at log2(B) − 2 ≈ 4/20 probes at B=64, ~0 at B=4.
  the earlier "30% off" estimate was wrong.
- **the single-lookup loop was never the naive baseline**: its independent
  iterations overlap in the OOO window (measured 1M single ≈ 322 ≈ 14-16
  effective misses at MLP-reduced latency, not 20 serial), and every
  single's top ~6 pyramid lines stay cached across the whole bench. the
  descent's spread-batch ceiling is that 322 × 0.8 ≈ 260 — and machinery
  eats most of it.
- DFS recursion serializes the child chains (1M spread: 663 = 2× single);
  the level-synchronous frontier (flat live list, one probe per active per
  round, loads of a round overlap) fixed the serialization (358 ≈ parity,
  avg-parity: 369 vs 366) but pays ~30-40 cycles/active/round of
  bookkeeping — at 10k (L2-resident, latency cheap) that's a 30% LOSS vs
  single (95 vs 74). DFS wins cache-resident, frontier wins DRAM; neither
  beats singles on spread keys.
- **the real prize is clustered batches**: range collapse dominates
  everything — B=64 adjacent = 23.8 (10k) / 51.4 (1M), 3-13× under single.
  both implementations pay the serial root chain (~log2(cap) dependent
  misses, the 1M floor ≈ 26ns/lookup at B=64).
- small B is structurally bad: no probe saving, full serial root, sort +
  frontier overhead — B=4 runs 2× single at 1M.
- open: machinery tuning (word-local bsearch state per active, fused
  compare+advance) has ~20% headroom at 1M; clustered-mid batches (64
  keys in a 4k window) unbenchmarked; explicit `_prefetcht0` of next-round
  probes unmeasured.

### the descent died; sorted serial won (same day)

three mechanisms got isolated by B=1 and the diagnostic cells
(`benches/sorted_lookup_batch.rs`: shuffled singles / batch cells /
globally-sorted singles / sorted chunks through `lookup_batch`):

- **B=1 = pure overhead ≈ 0** (72.3 vs single 72.3 at 10k, 329 vs 338 at
  1M) — the frontier machinery itself was free; the amplification lived
  in the round loops (~31 vs 18 cycles/probe), and attempts to shave it
  (mask fast-path, ping-pong compact buffers) both measured WORSE — the
  tight bsearch loop is too well scheduled to beat by restructuring.
- **a running lower bound is a trap twice over**: the loop-carried `lo`
  dependency serializes the batch (2× singles at 1M spread: 637), and
  even chunk-restarted (chain capped at 8) the bound DISPLACES searches
  off the shared top pyramid — chunk-8 168 vs 127 plain at 1M. bound
  inheritance is dead in this design; the frontier (median-partition,
  level-synchronous) died with it.
- **the real win is global probe order**: a fully sorted probe stream
  through plain full-range searches = 137 (1M) / 43 (10k) vs shuffled
  338 / 72 — 2.4× / 1.7×, with NO batch machinery at all: consecutive
  searches share probe paths (top pyramid + region lines stay hot) and
  independent iterations overlap in the OOO window. per-batch sorting
  (batch-sorted, globally shuffled stream) only buys parity — the
  monotone walk must span calls, which is the caller's, not the API's.

final `lookup_batch` = sort to pos order + serial full-range searches,
no bounds, no dedup (B=1 ≡ single; B=64 spread ≈ single, both scales;
**clustered batches still collapse via shared paths: adjacent 42 (10k) /
56 (1M), 6× under single**). the sort is the whole trick — the same
sorted stream through `contains` runs 137/43, so the batch API's value
over plain singles is the clustered case and B-independence, not spread.

### evens/odds two-phase — best design, still DROPPED (same day)

verdict: not worth the complexity. the wins are confined to clustered
batches (23.6/33.9 vs 42.3/56.1 plain — probe-path sharing does most of
that even without batching) and small margins at cache-resident scale;
spread keys — the dominant case — are parity with plain single lookups at
every B and both scales. the whole batched-lookup surface
(`lookup_batch`, two-phase, span rule, `benches/sorted_lookup_batch.rs`)
is deleted; the findings live here. the actionable residue for a real
caller: SORT YOUR PROBE STREAM — a globally sorted stream through plain
`contains` is the 2.4×/1.7× win, no API needed.

the batch sorted to pos order; even positions searched full, odd positions
searched within the span their even neighbors' spots bound. unlike a
running `lo`, the odds' bounds come from the evens' RECORDED results —
both passes dependency-free (no chain), and the saved-probe count is
~log2(B)/2 per element without recursion.

| ns/lookup | single | B=16 | B=64 | B=64 adjacent |
|---|---|---|---|---|
| 10k | 70.8 | 71.7 | 69.6 | **23.6** |
| 1M | 332.8 | 329.3 | 376.2 | **33.9** |

- always-bounded odds won 10k outright (69.7 vs 81.9 plain serial) and
  clustered at both scales, but lost 1M spread (470 vs 333): wide spans
  displace the odds off the shared probe pyramid — the same cache tax as
  `lo`, arriving by a different route.
- adaptive span rule: bound only spans ≤ ~L2 (512KB / size_of::<K>
  slots); wider spans search full. 10k (spans ~300 slots) keeps the win,
  1M spread returns to parity (B=16: 329), clustered keeps it at any
  scale (adjacent 33.9 at 1M — best number of every design tried).
  B=64 1M spread pays +13% (376): uniform keys leave ~40% of spans under
  the L2 line — bounded and displaced — the tax for serving mid-cluster
  workloads; retuning the threshold down would trade that away.
- clustered numbers across all tried designs (B=64 adjacent): plain
  serial 42.3/56.1, frontier 24/51, two-phase 23.6/33.9. the probe-path
  sharing does most of it; the evens' precomputed bounds do the rest.

### seqread engine round 2: in-place bit test (same day)

`SetBits`/`SetBitsRev` rebuilt per the second design: the current word
cached, each position tested in place (`word & 1<<(pos&63)`) — dense
streaks pay a bit test + increment, no tzcnt, no `(wi<<6)+tz` position
rebuild; holes advance one bit (tz-jumping in the else measured WORSE:
2.9 vs 2.0 on random — the jump machinery costs more than the retest for
scattered holes); whole-empty word tails skip at once. seqread ns/elem
best, vs the tz form:

| workload | tz | bit-test | 1M tz | 1M bit-test |
|---|---|---|---|---|
| append | 1.62 | **1.4** | 1.9 | **1.4** |
| prepend | 1.41 | 1.4 | 1.5 | 1.4 |
| random | **1.64** | 2.0 | 2.2 | 2.5 |
| runs | 1.62 | 1.7 | 2.2 | 2.3 |

dense blocks win ~15% (the position rebuild was the real cost — the test
+ increment replaces tzcnt + clear + shift + add); hole-y blocks (random,
~87% dense) lose ~25% at 10k (one else-path per hole), wash at DRAM.
kept the bit-test form: dense is the bulk-loaded index's steady state
(`from_vec`, tail-built blocks), and Desc/Asc now cost identically. a
tz/bit-test hybrid is strictly worse than both — the else path can't pay
for its own machinery (measured, not theorized).

### seqread engine round 3: rotating bit mask — no win (same day)

tried per the second sketch: a single-bit `mask` rotating one per forward
step, test = bare `mask & word` (no per-element `1<<(pos&63)` shift),
`pos` jumping only by ±64 at word boundaries, yield = `pos + tzcnt(mask)`.
10k best: append 1.6 / prepend 1.4 / random 2.0 / runs 1.8 — vs the
in-place form's 1.4/1.4/2.0/1.7. reverted.

the shift saved on the test reappears on the yield as tzcnt + add — in
the in-place form `pos` IS the position (yield is free, the increment
carries it); moving the position into the mask makes every yield pay the
rebuild instead. the op count is symmetric across the branch either way,
so the in-place form's zero-cost yield is the structural edge.

### seqread engine round 4: `#[inline]` — the call boundary was the cost

`#[inline]` on the per-element Iterator `next` impls (`SetBits`,
`SetBitsRev`, and the `Iter` enum dispatch in sortedblock): the consumer
is a separate crate, so `Iter::next` was a lib call per element (~1ns of
call + Option shuffling) — the dominant term the whole engine hunt was
fighting. with the loops collapsed into the consumer's body:

| ns/elem | round 2 | inline |
|---|---|---|
| append 10k | 1.4 | **0.4** |
| prepend 10k | 1.4 | **0.6** |
| random 10k | 2.0 | **0.8** |
| runs 10k | 1.7 | **0.7** |
| append 1M | 1.4 | **0.7** |
| random 1M | 2.5 | **1.4** |

0.4 ns/elem ≈ 1.4 cycles — the masked scan at near flat-Vec cost (0.2
L2 / 0.5 DRAM); at 1M the block reads 0.7 vs the Vec's 0.5, 12× BTreeSet.
the engine rounds 1-3 were tuning the body of a loop whose frame cost
more than its body. the fn-pointer unification was mooted by this.

### seqread session summary (2026-10-14)

the full engine ladder, ns/elem best (10k | 1M), from the first
measurement of the old `iter` (from_fn + `next_set`/`prev_set` per
element) to the final form (in-place bit test, `#[inline]`):

| | append | prepend | random |
|---|---|---|---|
| old iter | 1.98 \| 1.9 | 2.81 \| 1.5 | 2.02 \| 2.2 |
| final | **0.4 \| 0.7** | **0.6 \| 0.7** | **0.8 \| 1.4** |

~5× end to end. the ladder's rungs and their real contributions:
1. differential iterators + word-stepping (tz engine): killed `prev_set`'s
   keep-mask loop — Desc 2.81→1.41, the biggest single algorithmic fix.
2. in-place bit test (your sketch): dense −15% — the position rebuild
   (`(wi<<6)+tz` per yield) was the cost; `pos` as the state makes the
   yield free. rotation-mask and tz-jump variants: measured, no win.
3. `#[inline]` on the per-element `next`s: the round that mattered most —
   1.4→0.4. cross-crate call overhead was the dominant term the entire
   engine hunt was fighting; check this first, tune bodies second.

final seqread: 0.4-0.8 ns/elem cache-resident (~1.4 cycles/elem), 0.7-1.4
at DRAM — the flat Vec costs 0.2/0.5, BTreeSet 0.9/8.3. the masked array
now scans within 2× of bare memory at both scales.

### the bsearch got the same treatment (same day)

`next_set`'s dense fast path — the round-2 lesson applied at the probe:
when the bit at `from` is set (the ~90% case mid-probe), `from` IS the
answer — return it directly, no `>>off` shift, no tzcnt, no
`(w<<6)+off+tz` rebuild. `prev_set` is now test-only (the old iter was
its last production user). `#[inline]` on `contains`/`find` too — the
round-4 lesson — but bisected: inline alone ≈ nothing at 10k (70 vs 68 —
the call is amortized over ~14 probes, unlike iter's per-element call),
~10% at 1M. the fast path is the win:

| ns/lookup | before | both |
|---|---|---|
| append 10k | 68 | **57-58** |
| random 10k | 67 | **61** |
| runs 10k | 68 | **60** |
| append 1M | 328 | **256** |
| random 1M | 357 | **313** |
| runs 1M | 421 | **352** |

insert rides the same bsearch (spot-finding): random 1M 487→415, runs 1M
132→110, random 10k 96→92. no regressions anywhere; the 1M lookup
columns now stand ~256-352 vs BTreeSet's ~271 — parity-to-ahead of the
tree at DRAM scale in a single block (the earlier "the tree wins lookup"
story inverted at these sizes: 322→256).

### fast paths on the rest of the scans — kept, but insert unmoved (same day)

`prev_set`/`next_clear`/`prev_clear` got the same dense fast path as
`next_set` (the query position itself answers when its bit is the sought
one) + `#[inline]` on all scans. correct at every position vs the naive
models (the scans test sweeps every `from`). insert: flat within noise
(90.5/72.6/416.7/108.9 vs 91.7/74.7/415/109.7 across random/runs at both
scales).

why no insert win: the clear-finders are hole-finders — `nearest_clear`
queries from the insert SPOT, which is live ~90% of the time, so the
fast path (answer == query) fires only when a banked hole sits exactly
at the spot. the fast path's payoff case on next_set — the bsearch probe
landing on a live slot — is the 90% case there and the 10% case here.
kept anyway: zero measured cost on the scan paths (the fast path adds one
test before a scan that almost always follows), uniform across the scan
API, and `prev_set` has no production caller left either.

### nearest_clear excludes the center (same day)

contract tightened per the same shape as the fast paths: `nearest_clear`
searches from center±1 — `center` is guaranteed not to be the answer.
naive-model test updated to match.

this initially BROKE insert, and the break was instructive: the insert
spot is usually a HOLE, not a live slot — the bsearch converges `lo` to
the gap where k belongs, and `gather_slots` relied on
`nearest_clear(spot)` returning the spot itself for exactly that case
(`hole == spot` ⇒ no-op slide, k takes the open slot). the "callers pass
a live position" premise was wrong. fix: the spot-is-hole case is now an
explicit mask test at the top of `gather_slots` (one `get` — the same
check the old scan path performed implicitly), and `nearest_clear`'s
contract is honest: it can never return the one position the caller
already occupies.

no cost measured: insert 90.6/71.4/402.8/107.2 across random/runs at
10k/1M — at or under the previous run. the explicit bit test replaces
the work the scan did implicitly.

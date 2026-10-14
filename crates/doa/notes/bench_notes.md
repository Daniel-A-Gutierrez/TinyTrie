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

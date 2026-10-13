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

## related (earlier, separate bench)

`benches/sorted_find_slot.rs`: occupancy-mask find vs `VecStore` slot scan
— mask 1.28× (budget 16) / 1.45× (budget 320) at 80% occupancy, 320 slots.
density sweep at 90–97% still unwritten.
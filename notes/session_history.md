# session history

the WORKSPACE's working history (crate sections pre-2026-10-28 are doa; newer
entries may cover any crate): the problem a session worked on, the approaches tried, what
worked and what didn't, and why — plus the numbers that chose between them.
each crate's CLAUDE.md carries its living map (concepts, contracts, current state, future work); anything
temporal belongs here, not there.
devlogs are the user's own writing — sessions never write to them or point readers there.
one entry per session, appended by the session-end routine, newest last; the format contract
is the section below.

## entry format

`## YYYY-MM-DD — <short title>` (same-day sessions get (a)/(b)):

- `landed:` user-visible surface changes (API, module layout,
  invariants) — one clause each.
- `died:` deleted things + the one-line why — the do-not-revive record.
- `decided:` design forks taken, each WITH the rejected alternative.
- `measured:` bench numbers are HIGH-PRIORITY info and go in tables — columns are the
  comparison axes, the lead-in sentence carries the conditions (workload, scale, best/avg,
  partial-rep markers, machine-day caveats); single numbers can stay in prose.
  unbenched changes say "unmeasured".
- `deferred:` one-liners — the actionable item lives in CLAUDE.md's
  Future Work; the entry only records it slipped past this session.

style:

- past tense, verdict first; ≤ ~20 lines of PROSE per session — tables and the died list ARE
  the record, not padding, and don't count against the cap.
- no choreography narrative — mechanism lives in the code + doc/ skeletons and the
  current-state prose in CLAUDE.md.
- never restate what the diff or a conceptual section already says; when prose overflows the
  cap, compress harder — drop anything that is not a do-not-revive item, a decision, or a
  number.
- the devlog is the user's writing and is never written or pointed to by sessions.

formatting:

- each sentence starts on a new line; wrapping targets ~100 characters, but the newline
  before a sentence wins over the wrap width.
- more than ~3 items of the same kind become a bulleted list — visual hierarchy over inline
  enumeration.
- hierarchy means MORE THAN ONE TIER: when a bullet's content is itself a list (sub-items,
  per-design rejections, per-condition findings), nest bullets under it — never inline a
  big list inside one bullet.
- consider the purpose of a passage before writing it: instructions instruct, history
  records — never mix the two in one passage.

---

## 2026-07-11 → 07-24 — treeblock-era benches (legacy `bench notes.md`)

pre-blockset record, carried over nearly verbatim — the only record of the
translator fn-ptr numbers, the spread approaches, and the AoS-vs-SoA
layout bench (BlockMap's design ancestry).

- measured (find_slot, treeblock store, two-phase find, ns):

  | case | stride 2 | stride 8 |
  |---|---|---|
  | rank-1 hit | 6.6 | 7.2 |
  | rank-14 far hit | 12.7 | 13.3 |
  | append | 13.4 | 13.5 |
  | prepend | 12.1 | 12.3 |
  | full-budget miss | 16.0 | 15.0 |

  prepend ~budget/2 probes.
  cold-cache stream 4,655 ns per 256 inner iters (~18 ns/probe).
  baselines: O(len) scan 1.8–1.8k ns; outward scan with early exit
  8.9/95/2.6k ns (near/far/miss).
- measured (layout_bench — AoS vs SoA K→V lookup, Zen4 AVX-512, N=8/16;
  single-block probe, miss → full N-scan, ns):

  | variant | N=8 | N=16 |
  |---|---|---|
  | u64 AoS scalar | 1.47 | 2.25 |
  | u64 SoA scalar | 1.48 | 2.41 |
  | u64 SoA SIMD 512-bit | 6.99 | 5.46 |
  | u64 SoA SIMD 256-bit | 5.81 | 5.90 |
  | str AoS / SoA | 13.5 / 14.5 | 27 / 28 |

- measured (streaming, 1M blocks, hit at last position → scan + value, ms):

  | variant | N=8 | N=16 |
  |---|---|---|
  | AoS | 4.34 (~30 GB/s) | 8.40 (~30 GB/s) |
  | SoA scalar | 4.31 | 11.79 (~21 GB/s) |
  | SoA SIMD 512 | 4.11 | 10.97 |
  | SoA SIMD 256 | 4.35 | 12.48 |

- decided (layout): scalar beats SIMD 3–5× single-block — linear search
  with early-exit branches the predictor nails (~1 cycle/compare);
  SIMD's splat+cmp+movemask+tzcnt exceeds that whole sequence at N=8/16
  (`any()`-first a no-op; String pointer-chase bound, no lane mapping).
  AoS wins when hits need their value — it rides along free; SoA's val
  load is dependent on the key match, can't prefetch (N=16 30→21 GB/s).
  SoA wins only for miss probes / two-pass filter-then-fetch (half the
  bytes). SIMD pointless when memory-bound.
  untried: GLOBAL SoA (all keys one array, vals another) — a miss touches
  only keys, ~2× faster; per-block SoA denies that. → BlockMap's design
  ancestry.
- measured (address translation, ns/iter): the CPU does not elide runtime
  no-ops — all-zero params run the same ~1.66 ns/iter as all-nonzero (the
  no-op add/shl/ror sit in the critical v chain; only compile-time zeros
  are free).
  three shapes (all-zero / all-nonzero):

  | shape | all-zero | all-nonzero |
  |---|---|---|
  | straight (uniform math) | 1.625 | 1.628 |
  | branchy (guard each op) | 1.434 | 1.626 (50/50 pattern: 2.037) |
  | fn-ptr (specialized body) | 1.241 | 1.686 |

- decided (translation): fn-ptr specialization — pre-baked body per param
  shape, dispatch once at set_params; no per-iter branch, no mispredict
  risk, ~24% faster all-zero, ~+0.06 ns indirect call on real work.
  branchy rejected: a 50/50 alternating param pattern mispredicts every
  iter (+0.40 ns).
  const-generic inlining still beats fn-ptr for
  statically-known strategies — fn-ptr is the adaptive tier's shape.
  → treeblock's translator.
- measured (spread bench, VecDeque m=16384, spread-only µs):

  | approach | contig | wrapped |
  |---|---|---|
  | R pushnone (resize_with + reverse) | 30.3 | — |
  | U interleaved_halving | 25.3 | — |
  | U' interleaved_reverse | 27.1 | 22.6 |
  | U'+slice (make_contiguous) | 19.4 | 25.8 |

- decided (spread): push_back-interleaving places the upper half in
  final position without the wasted even-slot None-init — the safe-push
  equivalent of spare_capacity_mut (every slot written exactly once with
  its final value).
  reverse-move over halving (a wash, less code).
  branch on contiguity — make_contiguous is free contig (slice indexing
  skips the deque's (head+i)%cap, ~28% win), a net loss wrapped.
  caveats: m far bigger than real blocks (cap 8–256), dense all-Some
  input — direction robust, magnitude unverified at real sizes.
- landed: `DequeStore::spread` = interleaved phase1 + contig-branched
  phase2.

## 2026-09-19 — walker-surface refactor

- landed: the crate no longer drives tree mutations — it supplies OPEN SLOTS
  at ordering-correct anchors (`open_here`/`open_child`/`open_2_child`; preorder
  adds `open_parent`/`open_parent_child`, in-order adds `rotate_left`/
  `rotate_right` — physically free, walker survives); opens return
  `(slot(s), OpenFixups)`; all wiring (inserts, drains, reparenting, root
  promotion) consumer-side, against the block directly.
- landed: consumer mask flattened — `NodeCursor` (reads + consumer-impl'd
  `descend`), `NodeWalkerMut: Fixable` with `Snapshot`/save/load/set_position +
  set_child/clear_child/set_parent, `lookup -> Option<ChildPos>` (None =
  descent terminates — the consumer's equal policy), `children() -> (ChildPos,
  A)` pairs, DoubleEnded+ExactSize.
- landed: block surface — `TreeBlock::set_root`, `split_block(self, l, r)`
  (popped root + prefix split boundary; body `todo!` — arena tier),
  `BlockOps::pin_pos` + `try_find_slot`/`try_find_2_slots` (scan-only probes —
  the split-vs-grow decision's space test).
- landed: find ladder full-len scan rung — found by the 240-key ascending
  torture: append-heavy growth densifies the store edge past the BIT_WIDTH
  budget while mid-span holes remain; forced spreads ran to shift-exhaustion
  (len 65536, occupied 142).
- landed: `examples/btree.rs` rewritten on preemptive consumer-driven splits —
  green: 100 keys, multi root promotions, all anchor kinds, in-run +
  out-of-run slides.
- died:
  - `State` assoc type + accessors, `parts`/`parts_mut`, `has_space`,
    `current_mut`, walker-level insert/remove_child, `SplittableNode`,
    `Node::Payload`, the `DEGREE >= 3` bound (binary rotations are legal).
  - `Suggested`/suggest fns, `hop_current`, postorder impls, `TreeWalkMut`,
    `SplitWalkHelper`/`SplitTreeWalker` (the old split-driver/hop/postorder
    torture died with them).
  - `TreeWalkHelper` (slide/open engines, `Anchor`) sealed `pub(crate)`.
- decided: save/load/set_position proposed for deletion and KEPT after review —
  fixup's correctness instrument (the run walk rewrites entries to post-slide
  addrs, so walking back is unsound, subtle_bugs §5); the per-slide clone is
  an optimization target, not a subtraction.
- decided: set_child/clear_child/set_parent kept by decision; held addrs
  never survive an open uncorrected — consumers apply the returned fixups.
- measured: unmeasured (correctness session — `tests/walker.rs` live on the
  new surface, miri-clean).
- deferred: `split_block` body; store tests port.

## 2026-09-19 (b) — rotations/gather machinery

- landed: rotations → the stand-on-the-riser contract (agreed direction):
  the walker starts ON the riser, all writes via up-navigation over the
  ancestry stack, ends ON the riser with the ancestry popped ONE level — the
  riser ROSE, so the end state must be TREE-TRUTH, not position-only "ends
  where it started". Root/mid-tree uniform; root case ends with an empty
  stack.
- landed: §4b partial — `set_parent -> Option<B::A>` (the overwritten old
  parent addr, None for parent-free shapes); crate call sites discard it.
  `Ancestry` inline-backed (8 entries + spill) — fixup's per-slide
  snapshot/restore is a fixed-size copy below depth 8.
- landed: gather machinery v1 —
  - `GatherSlide` (the N-None gather as a closed-form CROSSING-COUNT fixup,
    NOT sequential `NoneSlide`s: overlapping runs go stale mid-application).
  - `Store::find_n_slots` (rel-side nearest-n, pin-clamped) + per-store
    `gather_none`.
  - the `BlockOps::find_n_slots` ladder (budgeted → full-len →
    spread+rescan → exhaustion).
  - walker `fixup_gather` + `apply_gather`.
  - `open_n_at` + `open_n_here`/`open_n_child` on both orderings.
  - `OpenFixups.gather` (no longer Copy), `Fixable::gather_fix` (by-ref).
  - v1 scope: holes all on the open's rel side; Pluripotent default ladder
    (no edge-grow for n>1); Uniform lacks the proactive 3/4-spread rung.
- died: the old MID-CHOREOGRAPHY reach (walking to the riser through a
  half-rotated tree) — replaced by a state-only `ascend` + `set_position`
  restore over the fully-consistent post-rotation tree.
- decided: the riser's own stored-parent-field write kept (the original
  pseudocode omitted it; parent-free shapes no-op it).
  the `Ancestry` clone
  stays — irreducible for stack walkers (§5: popped anchor-path entries
  can't be re-derived mid-fixup); the heap alloc was the cost.
  `set_parent`'s `Some` returns unexercised (no parent-field consumer
  exists).
- landed: tests — rotation tests reworked (end-STATE truth asserted: is_root
  at the root, the true parent mid-tree), `pre_open_n_after`/`_before`/
  `_before_adjacent`/`_deep`; 12 tests, miri-clean.
  in-order `open_n_*`
  untestable against the binary consumer (a 2-node burst needs a slot on
  each side of the parent gap, else leftless-right chains) — waits for a
  DEGREE>2 in-order consumer.
- fresh-review: real bugs fixed pre-commit —
  - the Before member interval stopped at anchor−1 instead of the anchor
    (tripped the canary).
  - the rotation end state kept the stale descent entry (is_root/parent
    lied).
  - the Before reparent range underflowed on the empty-member edge.
- measured: unmeasured (correctness session).
- deferred: §4b walk-back usage (ascend-by-old for parent-field walkers —
  the return is in place, no consumer exercises it); store tests port +
  gather matrices.

## 2026-10-06 → 10-12 — module split + BlockSet insert live

- landed: module split — src/ = substrate (`index`/`Pos`, `translator`,
  `lib` wiring) + `treeblock/` (all tree machinery, byte-identical) + the
  set tier (now blockset); re-exports keep old paths stable.
- landed: set-tier store pairs protocol (10-06) — `open_slot` fused
  find+slide+record emitting swap-cascade `(from, to)` pairs (the treeblock
  fixup contract doesn't apply — set holders want plain pairs); gather/bulk
  still plan-returning.
- landed: BlockSet insert basics (10-12) —
  - bsearch over live slots (Ok = dup, Err = insertion spot).
  - `try_alloc` (nearest hole either side, fails clean).
  - in-place rotation-turn grow; slide = one `ptr::copy` + O(1) mask.
  - `Delta::NoneSlide` compact event with lazy cascade expansion.
  - `Drop` via `assume_init_drop`; cap 0→4 vec-like first allocation.
- decided: the set-tier translator settles as the 2-knob degeneration —
  (offset, rotation) alone can't survive width growth + post-rotation
  adjustment (carries break the rotation), but the set's address width is
  FIXED and growth IS the rotation turn.
- died: bitvec — miri-unclean (integer-to-ptr provenance in its own
  internals); replaced by `src/mask.rs` (the word form is what makes the
  scans fast anyway).
- measured (10k shuffled u64 inserts, best/avg ns): optimization ladder —

  | change | best | avg |
  |---|---|---|
  | fresh-alloc grow, bit-at-a-time hole scan, per-element slide | 385.7 | 431.5 |
  | in-place grow (descending 2·pos pass + word-wise mask spread) | 379.2 | 386.3 |
  | word-wise hole scan (`nearest_clear`/`next_clear`/`prev_clear`) | ~260 | ~267 |
  | slide = one `ptr::copy` + O(1) mask | ~123 | ~130 |
  | `Deltas` compact event (no per-pair Vec pushes) | **~86** | **~90** |

  per-pair delta recording cost ~149 cycles/insert — 2× the memmove it
  described; one contiguous run shift = one 16-byte enum write.
  - phase breakdown (rdtsc, scaffolding since removed, ~3.5 GHz):
    - bsearch ~231 (⅔ of insert)
    - hole find ~38
    - slide ~41
    - mint ~29
    - grow ~18
- measured (32B items, 10k keys, best-of-10, vs BTreeSet, ns):

  | workload | insert BlockSet | insert BTreeSet | lookup BlockSet | lookup BTreeSet |
  |---|---|---|---|---|
  | append | 10.4 | 29.3 | 64.4 | 41.0 |
  | prepend | 10.3 | 23.5 | 69.0 | 41.6 |
  | random | 90.2 | 50.4 | 68.4 | 41.0 |

  sequential insert ~2.5× faster (push_back + amortized Vec vs node alloc +
  rebalance); random BTreeSet 1.8×; lookup BTreeSet 1.6× (the bsearch
  cache story: ~14 probes over a ~128–320KB array); the structural answer
  is bounded block caps + separator routing, and BTS returns no stable
  names, no zero-churn growth, no flat serializable array.

## 2026-10-12 → 10-13 — deltas safety + the unified ladder

- landed: deltas safety — `Delta::Empty` explicit sentinel (pending ⇔
  event ≠ Empty), `insert` panics on pending deltas instead of clearing,
  `Deltas::clear` public; caller protocol: apply, then clear.
- landed: the unified ladder — the same rungs for every block:
  - strategy-free tail fast path (`spot`).
  - edge rule (`spot == cap` → `push_back` — Vec's own doubling carries
    cap).
  - then `open`'s rungs: BIT_WIDTH probe → back-open (push_slot + slide
    when the tail run is within budget) → dense turn (len ≥ cap − cap/5) →
    local turn (len ≥ cap − cap/3 AND nearest hole > 4·budget) → fluke
    wide slide → mask-full grow.
  - `insert` split: `spot`/`insert_first`/`push_back`/`open`/`place`.
- landed: `rotate_mask(offset, midpoint)` replaces `spread_double` —
  plain interleave + one masked ripple over the wrap band (prefix/suffix/
  whole/empty); `grow_rotate` refuses at cap > MIDPOINT; the +1 band is
  the recycled rol bit (subtle_bugs §19).
- died: `InsertBias` — split into (Direction, Growth), then Growth DELETED
  when the ladder unified.
- decided:
  - `push_slot` is the single growth primitive (grow_rotate is a layout
    change, not a capacity change — subtle_bugs §18).
  - the local-turn gate guards sparse blocks from cap-doubling on hot
    clusters (spreads never leave the block below ⅓); below the ⅔ floor
    the wide slide stands (accepted: space over slides).
  - the wrong-direction column (descending into Asc) is irreducible —
    every insert must move the prefix, slide/turn regimes amortize to the
    same order; its fix is Direction, not the ladder.
- measured (32B, ns/insert, best-of-10):

  | random | ns |
  |---|---|
  | lazy turn | 99.3 |
  | preemptive 80% | 88.7 |
  | unified | 87.0 |

  | sequential | ns |
  |---|---|
  | pre edge rule: Spread-under-sequential | 615 @32B / 225 @8B |
  | pre cadence fix: push_back | 64 |
  | after edge rule + cadence fix | 8 (both columns) |

  - pre edge rule: tail holes deplete — every append slides from a distant
    front hole, O(n) slide + ~n-pair cascade.
  - the cadence fix: the hole goes BELOW k — appends can't consume it;
    pre-fix the trailing hole was eaten every append, the block stayed
    dense, every push_back paid a full-mask prev_clear + doubled push.
  - element size amplifies the preemption win: −6% at 8B → −11% at 32B;
    turn amortization a wash by design (~2 moves/insert either way) — the
    win is bounded slides + short cascades; turns are the only zero-churn
    growth (the holder-side win exceeds a deltas-clearing bench).
  - the local-density gate: dormant for random (max gap ~26 at 67%
    density, under the 4·budget=64 trip), ~10% cost on wrong-direction.

## 2026-10-13 — fetch-gather + the reversal tier

- landed: the unified ladder died for fetch-distance — `FREE_STRIDE = 8`
  replaces budget/cadence: no cadence holes (push_back dense), no
  80%/⅔/4·budget turn rungs; `open` = preemptive turn past 7/8 when the
  insert sits > stride from the edge → `gather_slots` (fetch = one hole
  per stride traveled) → mask-full (one tail push within stride of the
  edge, else turn / refused tail slot).
- landed: `Delta::Gather(Vec<NoneSlide>)` — multi-fetch chains, lazily
  expanded to the same cascades; the single-slide fast path stays the
  alloc-free `NoneSlide`.
- landed: reversal tier — the set-tier translator extracted (fn-ptr per
  direction); `BlockSet::reverse` = data+mask mirror + `offset → cap−1−offset`
  + pos_sum reflection, ZERO deltas (names follow the mirrored slots —
  pending events survive).
  trigger `pos_sum·8 < len²` past a 4·stride warmup.
- landed: turns under ANY offset + reversal (the `offset ≠ 0` refusal
  deleted) — the reversed turn's band lands `2p−1`, a DOWN-ripple crossing
  word boundaries; the reversed turn requires `offset < cap`
  (subtle_bugs §20).
- landed: position/bsearch — direction dispatch outside the loop,
  monomorphized comparator per arm; no order branch in the hot loop.
- died: `Direction`/`InsertBias` (session 4's bias split) — the block reads
  direction off the translator.
- measured (32B, 10k, best, same machine-day as the ladder rework):

  | workload | old ladder | chained gather | one-pass | + 7/8 turn | + reversal |
  |---|---|---|---|---|---|
  | append | 10.5 | — | — | 7.6 | 8.5 |
  | prepend | 10.3* | — | — | — | 74 → **8.5** |
  | random | 93.5 | 1704 | 153 | 98 → 93.5 | 99 |

  *prepend's 10.3 rode the Desc bias (tail pushes); the comparable
  wrong-direction number was 692–705.
- measured (random column's story):
  - chained per-slide application was quadratic in fetch distance (1704 —
    each slide re-copied the whole run); the one-pass segment compaction
    fixed the mechanism (153), the 7/8 turn the policy (98; 80% vs 7/8
    measured identical — the line is not sensitive).
  - the `want = 1` fast path (hole within a stride, no Vec) recovered ~4ns
    (every mid insert was heap-allocating the hole list); banking itself
    costs ~2ns there.
  - the len/4 trigger flipped pure-random 4× (degraded to 822 — flipped
    blocks couldn't turn, the offset ≠ 0 refusal, since deleted); at
    len/8 + warmup 32: random flips 0, descending exactly 1, post-flip
    turning healthy (cap 10039 on 10k descending).
  - fn-ptr + trigger check ~1ns on the append hot path (7.6 → 8.5, part
    noise — BTB-predicted indirect call).
- measured (sorted-vec crate + runs workload, same day; insert ns):

  | workload | BlockSet | SortedSet | BTreeSet |
  |---|---|---|---|
  | append | 8.1 | 11.9 | 30.2 |
  | prepend | 8.3 | 1092.5 | 23.3 |
  | random | 96.6 | 569.2 | 53.8 |
  | runs | 85.1 | 540.2 | 28.7 |

  SortedSet is the unbounded-memmove story everywhere except append (O(n²)
  suffix shifts); runs is the workload the block was built for, though
  BTreeSet still wins (runs land within-node — a 30-element node memcpy
  beats bsearch + slide + event).

  | | BlockSet | flat-Vec | BTreeSet |
  |---|---|---|---|
  | lookup | 68–71 | 13.0–13.1 | 42–43 |
  | seqread (ns/elem) | 2.0 | 0.2 | 0.9 |

  the per-element mask scan + fn-ptr p2a is the price of mid-insert in a
  single 10k block (the structural answer, arena tier, unchanged).
- measured (seqread iter rework, same day): the `from_fn` + per-element
  `next_set`/`prev_set` iter died for the differential `Iter` enum (Asc/Desc
  picked once) over word-stepping engines;
  2.01/2.80/2.02/2.02 → 1.62/**1.41**/1.64/1.62 ns/elem — killing
  `prev_set`'s keep-mask loop was the biggest fix; remaining gap to the Vec
  = the position rebuild + enum dispatch.
  report-only: a run-based
  `iter_set` (yield occupied runs) would amortize the scan word-wise —
  unmeasured.
  (hoisting hazard in the bench guarded: numbers identical
  unguarded — the Vec's 0.20 is real.)
- fresh-review: no correctness bugs; the mixed-op randomized torture
  promoted inline.
- deferred: the 7/8 turn fires before checking whether the spot is already
  open — delete-max + reinsert-max pays a full turn for a free slot
  (nearest-hole-within-stride guard the candidate, unbenched).

## 2026-10-14 — bench scale-out, seqread engine, lookup fast paths

- landed: bench rework —
  - `sorted_insert.rs` takes a size CLI arg (default 10k; addrs widened
    u16 → u32 — u16's name space caps at 64k).
  - a 3s wall budget per structure×workload cell (slow cells finish one
    partial rep, reported over the completed prefix, `[partial done]`-
    marked — SortedSet's O(n²) columns stay benchable at 1M).
  - a runs workload (100-key chunks, half reversed, shuffled).
  - seqread + lookup columns.
  - the `sorted-vec` crate as a third column.
- landed: seqread engine, 3 more rounds — in-place bit test (cached word,
  `pos` as the state so yields are free — the position rebuild was the
  hidden cost), `#[inline]` on the per-element `next`s.
- measured (in-place bit test, 10k): dense −15%, hole-y blocks lose ~25%,
  wash at DRAM; kept — dense is the bulk-loaded index's steady state.
- died (never landed, both measured): a tz/bit-test hybrid — strictly worse (the
  else path can't pay for its own machinery); rotation-mask and tz-jump
  variants — worse (the shift saved on the test reappears on the yield as
  tzcnt + add).
- decided: `#[inline]` was the round that mattered most — cross-crate call
  overhead was the dominant term the whole engine hunt was fighting; check
  the call boundary FIRST, tune loop bodies second.
- landed: bsearch fast paths — `next_set` returns the probe position
  directly when its bit is set (the ~90% case — no shift/tzcnt/rebuild);
  the other three scans got the same fast path + `#[inline]` (flat on
  insert — hole-finders' queries are dense by construction, the payoff case
  there is the 10% case; kept for API uniformity at zero measured cost).
  `prev_set` test-only (the old iter was its last production user).
- landed: `nearest_clear` excludes its center by contract (callers pass a
  position they occupy) — initially broke insert: the insert SPOT is
  usually a HOLE (the bsearch converges lo to k's gap) and `gather_slots`
  relied on nearest_clear returning the spot; fixed by an explicit
  spot-is-hole mask test at the top of `gather_slots`.
- died: the whole batched-lookup surface (`lookup_batch`, two-phase, span
  rule, `benches/sorted_lookup_batch.rs`) — spread keys (the dominant case)
  are parity with plain singles at every B and both scales; clustered
  batches win 3–13× but probe-path sharing does most of that WITHOUT any
  API — a globally sorted probe stream through plain `contains` is the
  2.4×/1.7× win (SORT YOUR PROBE STREAM — the monotone walk must span
  calls, which is the caller's, not the API's).
  not worth the complexity —
  do not revive.
- died (the batched hunt's five designs, all measured):
  - median-partition descent — DFS serializes child chains: 1M spread
    663 = 2× single.
  - level-sync frontier — fixes the serialization, 358 ≈ parity at 1M, but
    pays 30–40 cycles/active/round — a 30% LOSS at 10k.
  - serial+running-lo — the loop-carried bound serializes AND displaces
    searches off the shared top pyramid; bound inheritance dead in this
    design.
  - plain sorted.
  - evens/odds two-phase + adaptive span rule — the best design, adjacent
    B=64 23.6/33.9 — still dropped.
  corrected model: the descent saves ~log2(B)−2 probes, not log2(B);
  singles were never the naive baseline (OOO overlap + shared probe pyramid
  near the floor).
  open: machinery tuning ~20% headroom at 1M;
  clustered-mid batches + explicit prefetch unbenchmarked.
- measured (1M scale, BS/SS/BTS, ns, * = partial prefix):

  | insert | BlockSet | SortedSet | BTreeSet |
  |---|---|---|---|
  | append | 12.8 | 22.6 | 53.2 |
  | prepend | 11.4 | 20700* | 15.6 |
  | random | 487* | 14753* | 213.8 |
  | runs | 131.9 | 13983* | 37.2 |

  | | BlockSet | SortedSet | BTreeSet |
  |---|---|---|---|
  | lookup | 328–421 | 256–263 | 299–301 |
  | seqread (ns/elem) | 1.5–2.2 | 0.5–0.6 | 8.6–8.8 |

  - the cache cliff measured: block random insert 96→487, lookup 68→330
    (bsearch probes walk 32–42MB, every probe an L3 miss).
  - BTreeSet degrades gently (lookup 42→299) and won 1M lookup
    pre-fast-paths — the arena tier's separator routing the structural
    answer, unchanged.
  - runs is the block's scale-proof workload (132 at 1M vs 85 at 10k —
    locality keeps slides fetch-bounded).
  - seqread scale-invariant for the block.
  - append/prepend barely moved (8.5 → 12.8/11.4 — tail pushes + the
    reversal tier are cache-slope-immune).
- measured (seqread old iter → final, ns/elem, 10k | 1M):

  | workload | old 10k | old 1M | final 10k | final 1M |
  |---|---|---|---|---|
  | append | 1.98 | 1.9 | **0.4** | **0.7** |
  | prepend | 2.81 | 1.5 | **0.6** | **0.7** |
  | random | 2.02 | 2.2 | **0.8** | **1.4** |

  ~5× end to end; 0.4 ns/elem ≈ 1.4 cycles, within 2× of bare memory
  (flat Vec 0.2/0.5, BTreeSet 0.9/8.3).
- measured (bsearch fast paths): lookup 10k 68→57–61, 1M 328→256–313
  (inline bisected: alone ≈ nothing at 10k — the call amortizes over ~14
  probes; ~10% at 1M); insert rides it (random 1M 487→415, runs 1M
  132→110).
- fresh-review: one real bug — iterator re-poll instability (stale cached
  word after exhaustion yields OOB positions; subtle_bugs §21,
  `iterators_none_stable` regression test).

## 2026-10-15 (a) — bsearch optimization

- landed: search surface restructure — the rev branch moved into `bsearch`
  (position is no longer a dispatch layer);
  - `bsearch` = the raw boundary (Err(lo), hole ok — spot's search).
  - `position` = bsearch's Err mapped through `next_live` (a LIVE target
    or cap — find's path); its fn-ptr concern was moot — `locate` is
    monomorphized per arm already.
  - `spot` calls bsearch directly (hole spots preserved — position's
    mapping would forfeit them, the banking test is the tripwire).
- landed: the L2 shape split —
  - `search_spec` (branchy early-exit loop: the next probe's load issues
    ahead of the compare, overlapping the miss) for DRAM-resident blocks.
  - `search_cmov` (fixed loop, no early exit, Equal narrows as Greater)
    for cache-resident.
  - dispatch on `data.len()·size_of::<K>()` vs ~L2 (`L2_BYTES` = 512KB —
    the same line odd_search's span rule drew).
  - Err(lo) boundary semantics UNCHANGED — a hole lo is free space gather
    exploits.
- decided: the 10-12 verdict ("bsearch left alone deliberately" — the cost
  is memory latency, not instructions; eytzinger/branchless only pay
  cache-resident; lookup won structurally via bounded blocks + separator
  routing) reversed, conditionally: branch-vs-cmov is a cache-residency
  tradeoff, not a universal win — core's fixed-loop note holds for
  cache-resident slices only, and here it drew the L2 line instead.
- died (all measured):
  - linear tail (cmov-accumulated spot once hi−lo ≤ 64) — 3× regression at
    10k (lookup 57→157: the per-position `mask && not-Less` chain compiles
    to data-dependent branches, mispredicting ~every position on random
    keys), worse at 1M (451 vs 313), and broke hole-boundary semantics
    (the banked-hole test caught it live).
  - `Equal => lo = p` + final compare — provable infinite loop (lo pinned
    on the dup, live successor at p+1: range [p, p+1) probes mid = p
    forever; no-early-exit requires Equal to narrow as Greater).
  - `Equal => hi = p` + shared final compare — measured 1M 235→381 append /
    291→533 random: with the early return gone the two-way match compiles
    to cmov and `search_spec` BECOMES `search_cmov` — the Equal early exit
    is LOAD-BEARING in the DRAM shape, documented on the fn.
  - within-search hybrid (branchy head + cmov tail, CMOV_SWITCH=256) —
    dominated: 1M pure spec 242/252/299/312 → hybrid 385/373/454/495 (the
    tail probes of a cold probe stream are first-touch misses; the
    crossover needs ~90% tail hotness the block cannot see, and even a
    2-probe tail costs ~80ns cold).
  - the precomputed-trip-count unroll — dead on arrival (`next_live`'s
    hole jumps make each iteration's range reduction data-dependent).
- measured (32B, lookup best, ns, residency split):

  | workload | 10k | 1M |
  |---|---|---|
  | append | 36.0 | 240 |
  | prepend | 35.5 | 244 |
  | random | 55.7 | 293 |
  | runs | 46.3 | 315 |

  fixed cmov alone: 10k 57→36 (−37%, the old 3-way match mispredicted
  every probe) but 1M 313→495 (+58%, the cmov bound serializes each
  address on the previous compare's resolution while the branchy loop
  speculates) — hence the split; 1M beats even the old baseline (256–352).
  insert rides it (random 1M 415→316).
  at 10k BlockSet beats BTreeSet on
  append/prepend lookup (36 vs 43) — "beats btree set about half the time
  now at lookup".

## 2026-10-15 (b) — module rename + BlockMap

- landed: this file — CLAUDE.md's Status section + bench_notes.md + the
  July `bench notes.md` folded into one per-session record (their content
  is the entries above); CLAUDE.md keeps the living map only, and the
  session-end routine appends here now.
- landed: `sortedblock` → `blockset` (dir + file, matching the struct;
  wiring, benches, doc skeletons, CLAUDE.md, subtle_bugs repointed).
  new `blockmap` module: `BlockMap<K, V, A>` — two parallel `MaybeUninit` vecs
  (keys/vals) under ONE shared mask, pairs moving in lockstep, borrows
  blockset's translator + `Deltas` unchanged (event recording surfaced
  `pub(crate)`).
  API:
  - `insert(k, v, deltas) -> Result<A, (K, V)>`
  - `get(addr) -> (&K, &V)`
  - `value`/`value_mut` by key
  - `remove`/`remove_key`
  - `find`/`contains`
  - `iter` over `(&K, &V)`
  - 12 inline tests, miri-clean.
  - new bench `benches/blockmap_insert.rs` (sorted_insert's harness + a
    BlockSet same-run reference column).
- died: `Deltas` private-field event writes at the gather sites —
  `record_none_slide`/`record_gather` own them now (blockset's
  gather_slots refactored onto the same calls).
- decided: value slots are NAMELESS — one addr names the pair; no value
  channel in the deltas protocol (values ride the pos-space moves).
- measured (blockmap_insert; 32B Item keys, u64 vals, best-of, [p] =
  partial rep; vs BTreeMap, with BlockSet as the same-run set-tier
  reference):

  | insert | BlockMap | BTreeMap |
  |---|---|---|
  | append @10k | 8.8 | 30.8 |
  | append @1M | 12.3 | 58.0 |
  | prepend @1M | 12.1 | 24.9 |
  | random @1M | 331.8 [p] | 249.2 |
  | runs @1M | 123.7 | 46.7 |

  | | BlockMap | BTreeMap |
  |---|---|---|
  | seqread @1M (ns/elem) | 1.1–2.0 | 9.3–16.2 |
  | lookup @10k | 34–54 (except random) | 42–43 |
  | lookup @1M | ~300–350 | ~300–350 |

  - sequential insert 2–5× faster.
  - seqread 5–10× faster at 1M.
  - lookup parity — BlockMap wins at 10k except random, the 1M cache cliff
    flattens everyone.
  - random/runs insert LOSE 1.3–2.6× — BTreeMap's node splits beat
    whole-block moves; the set tier's profile, unchanged by the map tier.
  - the value vec costs ~5–10% on move-heavy inserts, ~0 on lookup,
    ~0.5ns/elem on seqread (vs BlockSet same-run).
- landed (later same session): the tiers promoted flat into src/ — `blockset.rs` +
  `blockmap.rs` top-level (the dirs and their mod.rs contracts gone; the files'
  `//!` headers already carried the invariants).
  the 2-knob translator is now `src/translator.rs` (substrate to both tiers); the 4-knob
  moved to `treeblock/translator.rs`.
  `sorted_find_slot` now runs its store side against treeblock's store.
- died (later same session): `blockset/store.rs`, the pairs-feedback copy of treeblock's
  store — unused by the tier itself (BlockSet's ladder is mask-driven inline); only its
  `NoneSlide` survived, shrunk to the from/to deltas-protocol link in blockset.rs.
- decided: the set/map tiers are the primary surface, so the `src/translator.rs` slot
  belongs to the 2-knob; treeblock keeps its own 4-knob translator in-module.
- landed (latest): `blockbimap.rs` — `BlockBiMap<K, V, A>`, the bijective tier: two lockstep
  BlockMaps holding each OTHER's names as values (devlog option 1 — both orders native,
  no projection ordering, one hop either side).
  insert churns both name spaces; `BiDeltas` carries both `Deltas` streams, and the maps
  consume each other's events by the BACK-POINTER CHASE: the moved entry's own value
  addresses the one partner slot whose held name went stale — O(1) per pair, no search.
  the fresh entry's A::ZERO dummy is never chased before its patch. 8 inline tests (holder
  mirror on BOTH name columns, mixed-direction streams, 600-op torture, dup-v rollback);
  miri on the small subset.
- decided (later same session): dup detection moved INTO the inserts, pre-check `contains` deleted —
  it paid a full bsearch whose answer `spot()` immediately recomputed (2 redundant bsearches
  per pair). a K-side Err is clean (the map detects pre-mutation); a dup-V Err rolls the
  just-placed k back — the slides it caused are real, so the bimap contract RELAXED: key-side
  deltas may be PENDING on Err, apply + clear like an Ok (a spot-as-hole placement records
  a no-op event, a push_back records nothing — pending depends on the k-placement path;
  the moved entries keep their new names, `keys.remove(k_addr)` punches only the pair's
  slot).
  measured after: 10k random insert 331.5 → 297.5 (−10%, the two saved bsearches), append
  41.8 → 38.6, prepend 29.9 → 26.8; @1M append 34.1 → 30.3, prepend 32.2 → 28.6 (1M
  random/runs cells too partial-noisy for before/after claims — the bsearch saving should
  dominate MORE at DRAM distance).
- died (never landed): `Deltas::iter_pos` + snapshot-translator exposure — TRIED, found
  load-bearing to avoid: consuming raw pos pairs skips the "translate then detranslate"
  round trip, but the reversal flip re-parameters the emitter's translator AFTER the
  event is recorded (names are era-stable, positions are not) — the name round trip is
  what re-anchors the read to the emitter's current slots (subtle_bugs §22; the chase
  reads `get(to)` via public `iter()`, the whole new surface is
  `BlockMap::set_value(addr, v)`).
- decided: the chase composes event pairs BACKWARD — the forward read assumed each pair's
  `to` is the entry's final name, true for plain slides but GATHERS CHAIN an entry through
  intermediate names (`to` becomes a later pair's `from`), and the emitter's final state
  has a different entry at the intermediate name (found live by `stride`; a `to` no later
  pair continues is the entry's final name, chains inherit their tail's resolution).
  ordering-by-projection deferred — option 1 needs nothing of it.
- landed (latest): full-hop gather pairs — `DeltaPairs::Gather` now walks the recorded
  slides' SEGMENTS and emits ONE pair per moved element carrying the crossing count
  (`w − segment index`) instead of chaining k 1-step slides — 24 pairs for the banking
  test's gather where 96 chained before. hop pairs OVERLAP (froms/tos share names when
  hop < run length), so emission walks away-from-spot (right descends, left ascends,
  far segment first) — every `to` stays ahead of the remaining `from`s or a first-match
  holder retargets one entry twice.
  consequence: the bimap chase collapsed to a single forward pass (`get(to)` per pair,
  one set_value per moved entry) — the backward flight composition died with the chain
  encoding (subtle_bugs §22 resolution).
- decided: the flip stays post-record — moving `maybe_reverse` to insert's top would
  make pos pairs sound for immediate consumers, but consumers read through names
  (era-stable, the protocol contract) and the trigger is a benched heuristic;
  recorded as Future Work, not worth perturbing.
- landed (latest): the flip moved to insert's TOP (blockset + blockmap) — the one
  post-record translator mutation is gone; the trigger now sees the pre-insert signal
  (fires one insert later; a heuristic, unbenched delta). the chase went POS-space:
  `iter_pos` + `value_at_pos`/`name_at` — no per-pair p2a+a2p round trip, reads hit the
  emitter's slots directly (subtle_bugs §22's rule refined: pos pairs valid from record
  to the emitter's next mutation; names stay the era-stable public contract).
- landed (latest): `benches/bimap.rs` — BlockBiMap vs a bidirectional control (two std
  BTreeMaps, K→V + V→K), sorted_insert's harness; cells insert (both sides), lookup by
  key, lookup by value, remove (both sides, shuffled order), iter_pairs; 32B Item keys,
  u64 values (v = 7k+3).
- measured (bimap bench, best-of, [p] = partial rep; control = 2×BTreeMap):

  | insert @1M | BlockBiMap | 2×BTreeMap |
  |---|---|---|
  | append | 73.2 | 124.5 |
  | prepend | 71.3 | 47.8 |
  | random | 830.1 [p] | 352.2 [p] |
  | runs | 610.0 [p] | 90.4 |

  | @1M | look_key | look_val | remove | iter_pairs |
  |---|---|---|---|---|
  | BlockBiMap (append) | 338.7 [p] | 178.5 | 435.2 [p] | 2.8 |
  | 2×BTreeMap (append) | 323.2 [p] | 224.5 | 703.4 [p] | 16.5 |

  @10k: insert append 81.7/94.3, prepend 67.4/47.1, random 330.7/117.1, runs 449.9/63.4;
  lookups parity-to-slight-win (42/37 vs 44/40); remove 78-94 vs 126-132 — bimap wins
  everywhere; iter_pairs LOSES at 10k (2.4-2.8 vs 1.4-1.7 — the per-element partner
  chase) but WINS 5× at 1M (2.8-3.8 vs 16.5-18.5 — the btree's pointer-chased nodes at
  DRAM distance).
  findings: look_val < look_key for BOTH structures (the value side's 8-byte keys pack
  ~4× denser than the 32B items — cache density beats key width). sequential insert:
  the bimap pays its two pre-check bsearches + the chase and still beats 2 btree
  inserts on append; prepend the control wins (2×23.5 cheap btree inserts vs both
  blocks' front work + flip). random/runs insert lose ~2.3-6.7× — block moves vs node
  splits, the set/map tier's profile. remove: bimap wins both scales (bit clears vs
  two btree removes). no flip-interference anomaly observed.
- landed (latest): `find` extremes fast path (blockset + blockmap) — beyond either extreme ⇒
  absent, two compares instead of a bsearch; the common sequential probe (append/prepend
  dup checks) lands past the tail. found by the bimap bench: append insert @1M was 73.2
  with 2×12ns of push_backs in it — the two `contains()` pre-checks were paying full
  bsearches that `spot()`'s own tail fast path already skipped inside the inserts.
  after: append 73.2 → **34.1** (≈ 2×12 + the patch, as predicted), prepend 71.3 →
  **32.2**; @10k 81.7 → 41.8, 67.4 → 29.9; random/runs unchanged (in-range probes —
  the fast path correctly doesn't fire). shuffled-present probes pay 2 mask scans on
  the miss; the 1M lookup cells moved +20-30ns on the UNCHANGED control too — ambient
  noise, no regression signal.
- measured (decomposition, scratch crate outside the repo — BlockBiMap vs TWO independent
  BlockMaps driven with identical streams, isolating the chase+patch+wrapper tax; 1M, best-of-3
  full reps):
  append insert 33.9 vs 25.8 — tax **8.1 ns/pair**: the additive story holds (2 blockmap
  inserts + wrapper ≈ the harness's 29.6-30.3).
  random insert 1249.0 vs 1004.2 — tax **244.9 ns/pair**: the chase is O(moved elements) —
  the same order as the slide work itself — a ~24% tax on top of 2× the block's slide cost.
  random lookup: get_by_key 668.5 vs the keys map's `value` 439.5 — **229 ns/probe**: the
  double-get structure (find→addr, `get` re-does a2p + re-reads, then the partner chase's
  two dependent lines — the partner's mask word + val slot).
- measured (contrast — why the bimap's win/loss RATES diverge from BlockMap's against the
  respective btree controls, @1M best-of):

  | cell | BlockMap vs BTreeMap | BlockBiMap vs 2×BTreeMap | divergence cause |
  |---|---|---|---|
  | insert append | 12.3 vs 58.0 (4.7× win) | 29.6 vs 125.1 (4.2× win) | none — additive, 8.1ns tax |
  | insert random | 331.8 vs 249.2 (1.33× loss) | 938 vs 553 (1.70× loss) | the 24% chase tax on 2× slide work |
  | insert runs | 123.7 vs 46.7 (2.6× loss) | 515.4 vs 92.7 (5.6× loss) | same chase tax, amplified — runs slide ~100-element runs in BOTH maps, the chase pays per moved element twice |
  | look_key | 285 vs 302 (parity) | 373 vs 328 (1.14× loss) | the 229ns double-get + partner chase — BlockMap's `value` reads its own parallel slot, no chase |
  | look_val | (no reverse column) | 206.3 vs 241.8 (win) | the values map's 8B keys pack ~4× denser |
  | iter_pairs | 1.1 vs 9.3 (8× win) | 2.8 vs 16.9 (6× win) | same mechanism — the chase's dependent loads pipeline (MLP ~30 outstanding lines → ~3ns/elem), the btree's node chain can't; the chase's +1.7ns of instructions only loses cache-resident (@10k 1.5 vs 2.4) |
  | remove | (unbenched) | 476 vs 741 (1.6× win) | 2 bit-clears + 2 bsearches vs 2 btree removes |

  the pattern: every divergence traces to the CHASE — it is the bimap's one new cost
  category, and it is proportional to moved elements (like the slides) on insert, a
  fixed ~2-dependent-line cost on lookup. the get_by_key double-get is the one fixable
  item: find-returns-pos + read-once (the same pos-space discipline the chase already
  uses) should reclaim ~130ns of the 229 — recorded report-only in Future Work.
- measured (memory, 1M entries, counting global allocator, live-bytes delta — scratch
  crate; B/entry, bold = smaller):

  | structure | dense build | random build |
  |---|---|---|
  | BlockSet | **33.7** | 67.4 |
  | BTreeSet | 63.6 | **50.6** |
  | BlockMap | **42.1** | 84.1 |
  | BTreeMap | 78.3 | **62.4** |
  | BlockBiMap | **50.6** | 101.2 |
  | 2×BTreeMap | 156.6 | **124.8** |

  the random-build INVERSION: blocks pay 2× for holes/turns while btrees get DENSER
  (sequential btree inserts split nodes half-empty — 78.3 inverts to 62.4). BlockMap
  LOSES random-build memory to BTreeMap; BlockBiMap still wins 1.24×; the dense-build
  3.1× win is the memory story, random-built blocks are not.
- measured (the 3/4 turn-line experiment — parametrized as `TURN_NUM/DEN`, tested,
  REVERTED; random workload, best-of):

  | cell | 7/8 | 3/4 | verdict |
  |---|---|---|---|
  | insert @10k (harness, full reps) | 297.5 | **243.7** | −18% — cache-resident, turns cheap |
  | insert-runs @10k | 389.7 | **357.6** | −8%, same story |
  | insert @1M bimap (full-rep scratch) | **1249.0** | 2815.9 | 2.25× worse |
  | insert @1M 2×BlockMap alone | **1004.2** | 1814.2 | the maps themselves +810ns |
  | insert @1M chase tax (bimap − 2×BlockMap) | **245** | 1001 | — |
  | memory @1M BlockMap (B/entry) | **84.1** | 168.3 | steady density ~25% |
  | memory @1M BlockBiMap (B/entry) | **101.2** | 202.4 | — |

  cause: the lower line doubles turn frequency, and each big turn's O(cap) relocation
  pass at DRAM scale eats the amortization.
  verdict: 7/8 stands — the 10k win is the cache cliff's near side; the line's job is the
  far side (session 5's 80% ≈ 7/8 at 10k was the same cliff seen from the other side).
  the four gather/rollback tests were re-tuned to sit below 75% density — valid under
  either line, kept.
- fresh-review (session end): NO correctness bugs — the reviewer verified the gather segment
  walker against a naive crossing-count property model (both sides, empty segments, the
  first-match emission order), the chase's no-post-record-mutation assumption on every
  insert path, and the find extremes' direction handling. findings applied: the "K-side
  Err mutates nothing" overclaim (the flip can fire on a dup-Err — orientation heuristic,
  invisible through the direction-aware API), the "dup-v always leaves kdeltas pending"
  overclaim (push_back placements record nothing), the stale `#[allow(dead_code)]` on
  blockset's `pos_sum`, the Testing section's dead test filter + stale miri-driver note
  (cargo miri works on the pinned nightly), bench-header "must divide 100" → "multiple of
  100"; test gap filled with `gather_left_side_hop_pairs` (left side, two non-empty
  segments, exact hop-pair assertions — the construction had to sit below the 7/8
  preemptive-turn line or the gather never runs). report-only perf, recorded in Future
  Work: `value`/`value_mut`/`remove_key` lack the extremes fast path.
- deferred: blockmap-specific tuning (none — inherited blockset's knobs
  unbenchmarked); `from_vec`; `entry`-style API.
## 2026-10-16 — cursors: seek + sequential iteration, all three tiers

- landed: the cursor family — lookup followed by sequential iteration, key-ordered,
  direction-aware. `BlockSet::seek(k)`/`seek_after(k)` (lower/upper bound) →
  `Cursor`: `next()` yields the current element then steps key-ascending, `prev()`
  steps back then yields, `key()`/`addr()` peek, `Cursor::seek`/`seek_after`
  reposition. `BlockMap` mirrors with `entry()` over pairs; `BlockBiMap::seek` →
  `Cursor` (key order) and `seek_by_value` → `ValueCursor` (value order) wrap the
  blockmap cursors and chase the partner one hop per yield (like the iters);
  `addrs() -> (K-side, V-side)` on both. raw-slot state — any mutation
  invalidates, re-seek. new bench `benches/cursor.rs` (find's harness: 4
  structures × {fwd = seek + 100 next, back = seek_after + 100 prev} × 4
  workloads, cursor-vs-range orders verified over a probe prefix; std has no
  stable cursor — `range(k..)`/`range(..=k).rev()` is the surrogate).
- landed: `Mask::step_up`/`step_down`/`word_at` — bidirectional cached-word
  steppers (the SetBits optimization pulled into the cursor): dense fast path
  tests the adjacent bit in the cached word (no load, no scan entry), same-word
  hops tz/lz-scan the remainder, crossings delegate to next_set/prev_set and
  return the fresh word.
- died: a third cursor ghost (`BackEnd`) — prev-exhaustion needs no ghost: the
  state stays at At(first), where every op answers what a resume ghost would.
- died: the `&mut u64` cache-parameter steppers — the pointer serialized the
  walk loop on store-to-load forwarding (see measured #1); value-passed instead.
- decided: cursor holds raw POSITIONS, not names — names are semi-stable
  (slides transfer them, subtle_bugs §17), a read cursor needs no fixup
  currency; documented invalidation instead of a deltas-applying cursor.
- measured (best ns/walk = one seek + ≤100 yields; 32B Item keys; the
  optimization ladder, each step benched at 10k):
  1. `&mut` cache param: append fwd 125→273 — WORSE. the old next_set's
     independent same-address L1 load was effectively free; the cache's
     read-after-write through a pointer is a serialized chain.
  2. value-passed: 273→220, still 2× — disassembly showed `Cursor::next`
     OUTLINED (both directions' crossing scans pre-inlined into it blew
     LLVM's size budget): the walk loop paid a call + cursor spill per step.
  3. `#[inline(always)]` on the walk methods: 220→134-173, and the back-walk
     asymmetry closed (append 242→177, random 392→235) — the rationale is
     documented on `Cursor::next` in blockset.

  final, pre→post optimization:

  | 10k (L2) best | BlockSet | BTreeSet | BlockMap | BTreeMap |
  |---|---|---|---|---|
  | fwd, pre | 125-218 | 197-244 | 150-232 | 202-219 |
  | back, pre | 235-392 | 188-220 | 237-407 | 224-244 |
  | fwd, post | 134-225 | 180-210 | 154-231 | 204-238 |
  | back, post | 177-235 | 190-230 | 185-251 | 223-264 |

  | 1M (DRAM), post | BlockSet | BTreeSet | BlockMap | BTreeMap |
  |---|---|---|---|---|
  | fwd | 577-859 | 1712-2083 | 678-1029 | 2063-2262 |
  | back | 653-969 | 1869-2129 | 705-1118 | 2065-2408 |

  - 1M: 2-3× over btree range on every cell, both directions — the walk is a
    contiguous scan once the seek lands; the btree pays node hops per step.
    back improved 10-13% over the pre-optimization steppers (append 755→653,
    random 939→865, runs 1056→969); fwd flat — the bsearch + DRAM dominate.
  - 10k post: every doa cell beats its std counterpart; pre-optimization std
    won all four back cells (the own-fwd/back asymmetry was 2×: append 125 vs
    242; btree fwd/back are symmetric because its cost is pointer-chasing
    either way).
  - miri: the cursor batteries sized down to 60-key sets (67-wide key space —
    every boundary still swept; coverage semantics unchanged, the flip still
    trips, gathers still bank) + the 70-bit stepper test: 31min → 2m14s,
    clean. the naive-battery-vs-volume split is the lesson — per-boundary
    exhaustiveness is what the tests are FOR; the key space can be small.
- deferred: `seek`/`seek_after` lack find's beyond-the-extremes two-compare
  fast path (report-only review finding, unbenched); bimap's cursor wrappers
  are plain `#[inline]` over `inline(always)` walks — one potential outlined
  call per yield, unmeasured.

## 2026-10-16 (b) — scan bench + MaskCursor: the walk machine moves into mask

- landed: `MaskCursor` (mask.rs) — the slot-walk state machine (slot /
  ghost / dir / word cache) that blockset's and blockmap's cursors wrap;
  the tiers keep key-order semantics only (seek landings, key/pair reads,
  `addr`). the u64 word no longer leaks into tier cursor state.
- landed: scan rows in `benches/cursor.rs` — full-set `iter` (SetBits/
  SetBitsRev) vs cursor fwd (seek min + `next`) vs cursor back
  (seek_after max + `prev`), checksum-gated, 10k + 1M.
- died: `CursorSlot` — the pub(crate) state enum both tier cursors
  duplicated; the ghost/walk's-start vocabulary lives in mask beside the
  steppers now.
- decided: `prev`'s ~1.4× over `next` is inherent to step-then-yield —
  the yield slot IS the step output, so the key load serializes behind
  the step (next's load runs beside its step). the shape restructure
  (mirror next, no double Option wrap) benched equal-or-better and
  stays; a lookahead-slot cache was REJECTED — buys back one
  dependency at seek/ghost complexity. SetBitsRev (iter on a flipped
  block, 0.8–0.9) acquitted descending stepping — the direction was
  never the cost.
- decided: `advance(p)` takes the slot the caller read from `slot()`,
  and walk-path methods are `#[inline(always)]` end to end. the first
  cut (hint-inlined `advance`) got the chain outlined at the scan site,
  0.6→1.6 ns/elem (2.5×) — a real `call` in the objdump. subtle_bugs §23
  carries the rules, incl. the silent no-op substitution that briefly
  faked a "tried inline(always), no change" data point.
- measured: cursor full-scan ns/elem, best, 4 workloads, pre (flat
  cursors) → post (MaskCursor). iter cells moved ±0.2 on pure code
  layout luck; the cursor cells are the comparison.

  | 10k | iter | cur fwd | cur back |
  |---|---|---|---|
  | append | 0.6 → 0.4 | 0.6 → 0.5 | 1.1 → 0.8 |
  | prepend | 0.8 → 0.6 | 0.7 → 0.6 | 1.0 → 1.0 |
  | random | 0.9 → 0.8 | 0.8 → 0.8 | 1.2 → 1.1 |
  | runs | 0.8 → 0.6 | 0.8 → 0.8 | 1.3 → 1.1 |

  the regression cut (hint `advance`): cur fwd 1.5–1.8, cur back 1.4–2.0.

  | 1M | iter | cur fwd | cur back |
  |---|---|---|---|
  | append | 0.7 → 0.7 | 0.7 → 0.6 | 1.1 → 0.9 |
  | random | 1.7 → 1.5 | 1.7 → 1.5 | 2.1 → 1.8 |
  | runs | 1.4 → 1.3 | 1.5 → 1.4 | 1.9 → 1.6 |

  the walk cells improved too (the machine inherited prev's restructure
  and lost a state re-read): 10k append fwd100 155.8→115.7 ns/walk,
  back100 179.0→138.5; 1M runs fwd100 828.4→788.1. cursor parity with
  iter holds at both scales — the whack numbers in the seek+100 cells
  are the seek's bsearch + one cold cache line per walk, not the
  stepping; the beyond-extremes seek guard stays deferred from (a).
- miri: cursor batteries across all three tiers (8 tests, flipped blocks
  included), clean, 2m10s.
- deferred: nothing new.

## 2026-10-19 — archive tier + SkipSet

- landed: `serialize.rs` — the `Archive` trait (K/V decompose into a POD
  `Handle` in slots + bytes in an append-only side heap; `cmp_key`,
  `view` GAT, `materialize`); integers/char by macro, String (8-byte
  BE-loaded zero-padded prefix + ≤8-byte inline cutoff — no heap bytes),
  Vec<u32> (LE, 4-aligned). BlockSet is now `K: Archive` over handles;
  drop machinery removed (heap bytes leak on remove by design, v1). landed:
  SkipSet (`skipset.rs`) — deterministic skip list over BlockSet:
  tier0 WINDOW-FIRST sampling (slot j = first live item in window
  [j·32, (j+1)·32) via next_set — None only when the whole window is
  empty, so banking/turn bands/gridline holes can never empty a sample);
  upper tiers lift every 32nd slot below; ONE allocation for all tiers
  (regions [top…tier0], reservations cover one turn + the cap ceiling);
  incremental upkeep — slide chains coalesce to one union interval,
  tail pushes update a window's first iff the placed item IS the first,
  silent moves (epoch) re-read all windows, creep extends. SkipSet::insert
  owns reversal manually (should_reverse + reverse; the base never
  auto-flips under tiers) and drives `insert_at` (unsafe window-bounded,
  bounded bsearch inside the descent's [blo, bhi)) or `push_beyond`.
  landed: `reverse_due` (z-gated reversal: pos_sq_sum variance,
  2σ, multiply-only hot path); `slides()` (raw NoneSlide chain, replacing
  pair expansion); bench strings.rs (3-way string bench, 3 shapes) and
  skipset.rs (3-way + maintained cell + A/B phase ladder + turn/slide
  census + desert census).
- died: rank-sampled rebuild-upkeep tiers (v0 — every rank shifts);
  per-tier Option<Vec> storage (one masked store); rebuild-on-every-insert
  upkeep; the j→2j interleave + LastMove translator-pair remap
  (window-first made re-mapping moot — the block knows firsts, slots
  don't); next_sample cache (const power-of-2 stride made modulo one AND);
  SkipSet's find_window dup pre-check (insert_at detects dups pre-archive);
  blockset get_owned/iter_named/find_in/find_below (superseded by
  slot/first_live_pos/find_window); the s² chain-stop misreading of the
  DEGREE² rule (root scan must stay ≤ stride).
- decided: window-first over gridline sampling — gridline slots go
  phantom-None exactly where banking is hottest; the first-item contract
  keeps `hi = 32·first_gt` exact (k < first(window w) ⇒ answer below w).
  decided: single allocation + region reservations over per-tier Vecs
  (one allocator call per epoch). STRIDE=32 as a module const (runtime
  divisors never compile to shifts — the div instructions were ~20× the
  harmonic tier share). 3/4 turn line re-benched: REJECTED at 1M (see
  measured). rkyv rejected: value-relative offsets vs our heap-relative
  handles; the free-lunch layer (serialized form == in-memory form) exists
  by design. two datastructure impls rejected: identity Archive impls
  monomorphize to today's code (int bench flat ±5%).
- measured (u64 keys unless noted, best (avg), 1M, same-run cells):
  lookup — BlockSet 329 vs SkipSet **126.5** (−62%); BlockSet vs BTreeSet
  329/180; append 121→103 (−15%); runs 174→164 (−6%). insert — maint
  append 12.4 vs base 8.6 (guards + amortized upkeep, harmonic share
  ~n/16 ops confirmed by census); random ~330 vs 282 base; runs 129 vs
  86 (down from 8× base pre-fix). seqread parity everywhere; BTree wins
  slide-heavy mutation (runs insert 33 vs 86) — node memmove vs scattered
  short shifts, deferred to block-of-blocks. strings (BlockSet<String>,
  1M): digits lookup 705→464 (−37%) vs owned-String; seqread −71%; the
  ≤8-byte cutoff pays (lookup −35% at 1M vs 9-byte keys). turn census:
  turns are 1.1–2.9% of build time at every workload/scale — the turn is
  never the cost, the LAYOUT is (3/4 line at 1M random: lookup 347→1132,
  moved/insert 13.7→68 — doubled cap → sparser probes + stretched banking
  deserts; runs moved 21.3→19.1, lookup 204→156 — per-workload bet).
  slide census at 1M random: 13.67 elems/insert — plain 87%×1.3 + gather
  13%×93.7 (count-bounded fetch `1+d/8` walks the hole chain: quadratic
  in desert depth; banking loses both bets — random never reuses, runs
  refetches); deserts: 75% of live sits in runs >8. phase ladder (A/B
  paired medians): descent +52 vs window-search −53 at 1M (cancellation
  is real) — upkeep is the entire visible tax (+5 appends, +12 runs after
  shift/mask + coalescing; pre-fix upkeep was +96/+303).
- miri: serialize, skipset, blockset clean (multiple runs incl.
  window-first); census counters #[cfg(test)].
- deferred: distance-bounded fetch (the gather tail — bound the walk,
  fall through to push_slot); off-grid banking (element-movement cost
  remains after window-first fixed the tier cost); blockmap/bimap Archive
  port; skipset removal (v0); auto-stride + L2 build gate (tiers lose
  below cache scale — 10k lookup −35%); heap vacuum; block-of-blocks
  (runs mutation).
- session-end review (fresh agent, no context): verified sound —
  insert_at's window contract (incl. the bhi clamp), window-first upkeep
  under every mutation class (union-interval coverage, no-op (p,p),
  push_beyond past tail holes, extend-before-slot-math ordering), epoch
  timing, z-gate reflection, serialize boundaries (7/8/9, NULs, LE).
  findings fixed: find_window's Asc FIRST probe was unbounded by `to`
  (empty window could return an out-of-window addr — pub contract, no
  in-crate caller affected); insert_maintain_only's dup-Err path could
  leave the epoch un-synced after a mid-insert auto-flip (bench-hook
  landmine); the pending-deltas assert moved to SkipSet::insert's top
  (was after the manual flip); reverse_due's spread now documents the
  no-removals assumption; turn_census's plain/gather split at ≥8;
  strings bench gained the lookup verify gate. declined: the
  redundant-apply_slide-after-sync_move flag (cold path, unbenched).

## 2026-10-20 — gather-heuristic rebalancing: the word-classified ladder

- landed: mask `copy_region` (region occupancy → fresh Mask, word-shift
  with carry) + `count_below` (prefix popcount — the boundary rank).
  `Delta::Rebalance { base, at, stride, mask }`: the region's live+1
  elements — k included at its rank boundary — redistribute onto the
  lattice `u(i) = i + i/(stride−1)`; `DeltaPairs::Rebalance` re-derives
  pairs from the pre-move mask in two passes (right-movers descending,
  left-movers ascending — first-match safe: targets are strictly
  monotone in rank, so a target's old inhabitant moved the same
  direction, earlier in its pass); `slides()` reports the region as one
  synthetic link (skipset's union upkeep unchanged). blockset's ladder
  rewritten word-classified: direct / NEAR (own-word hole; plain slide,
  or an in-word bank when the streak is hot and the hole is past
  FREE_STRIDE) / BORROW (a neighbor under High banks BORROW_MIN+streak
  holes) / the fork — cold streak rebalances, hot banks, both ending in
  `rebalance_region` (whole-block scan, sparser side first, tail-growth
  valve `push_slots`, name-ceiling slide fallback). `streak`/`last_insert`
  fields (locality signal; scaled ×2 on turns, reflected on flips).
  append fastpath in open(). `gather_slots` INFALLIBLE (returns Pos; the
  len/2 + REGION_WORDS bails, forced retries and Option plumbing deleted;
  the review agent falsified the growth panic's invariant in the
  turn-refused regime — now a slide). constants: FREE_STRIDE 8→6,
  scan stop 5/6 strict (= stride-6 lattice capacity), HIGH 53, BORROW_MIN
  10, TURN line 7/8→5/6 (ceiling = stop), STREAK_BANK 2. bench: workload
  name filter, maintained-path census (skip_census), blockset
  ladder_census (event-shape classification, DOA_CENSUS_N). skipset: a
  `#[doc(hidden)] base()` accessor. tests: 3 ladder tests + pair-safety
  battery; miri clean incl. the mixed torture.
- died: the count-bounded fetch ladder as the primary far rung
  (fetch_holes/compact_gather demoted to the bank arms — banking's
  desert-digging only ever bit scattered inserts, which now rebalance
  first); the distance (< FREE_STRIDE) near/far split (a banking
  artifact — rungs are word-classified now); single-hole borrows (one
  hole per frontier insert re-pays the full run — 79% of runs inserts ×
  ~54); decline rungs (mid-block bails, decline→turn, decline→fetch);
  the 7/8 preemptive ceiling; FREE_STRIDE's power-of-2 assert.
- decided: the fork — the streak decides balance vs bank (balancing wins
  random's layout, banking wins runs' amortization; measured both ways
  at 1M). k rides the (live+1) lattice so the rebalance IS the open —
  Deltas holds ONE event, no follow-up slide (the two-record overwrite
  bug this forces is why the lattice includes k). scan stop = the stride
  cap, NOT the sketch's 3/4: any stop below the block's steady state is
  KNIFE-EDGED (a leveled area sits just above 3/4 and can never average
  down — measured span-7522/8293 rebalances, every slot re-leveled
  ~790×/build); 7/8 had the same edge at the 76-84% steady state. ceiling
  = stop keeps density under workability, making the whole-block search
  self-terminating — and the wide target-to-ceiling gap measured
  favorable. borrow banks the lendable surplus (HIGH guarantees ≥ 11
  clear bits): one word-bounded event amortizes a cluster, a milked word
  lands at High and stops lending. the tail grows instead of declining
  (~1/7 of the region's live at worst, vs a turn's 2× doubling).
- measured (u64 keys, 1M, best, same-run cells, machine-day 2026-10-20;
  baseline = the count-bounded fetch ladder at HEAD):

  | | baseline | final | Δ |
  |---|---|---|---|
  | random insert (census build) | 346.7 | 191.2 | −45% |
  | random moved/ins | 13.67 | 3.64 | −73% |
  | random maintained | 301.7 | 162.4 | −46% |
  | random lookup base/skip | 326.2 / 115.3 | 142.4 / 125.4 | −56% / +9% |
  | random seqread | 1.2 | 1.6 | +33% |
  | runs insert (census build) | 194.5 | 212.9 | +9% |
  | runs moved/ins | 21.3 | 16.6 | −22% |
  | runs lookup base/skip | 188 / 155 | 213 / 172 | +13% / +11% |
  | runs seqread | 0.8 | 0.9 | parity |
  | runs cap | 1.74M | 2.27M | +30% |

  iteration ladder at 1M runs insert (census): baseline 194.5 →
  rebalance-first +49% (borrow-milking's per-hole cost) → banked borrow
  +40% → streak boost +49%→ banked-near/fork +14% → decline removal
  +12% → 5/6 ceiling +9%. random insert: −5% (rebalance) → −8% → −35%
  (2/3 family: density is the dominant variable, not the desert census)
  → −40% → −45% (ceiling 5/6). runs' final read regression is THRESHOLD
  LUCK: len 1M sits between the 5/6 (873k) and 7/8 (917k) turn lines of
  cap 2^20 — one extra doubling fires at 5/6 (cap 1.18M→2.27M). at 10k:
  runs −8% insert, movement −41%, reads parity; random parity on the
  census build, lookup +34% (cache-resident sparsity hurts — layout wins
  need DRAM scale).
- session-end review (fresh agent): verified sound — the two-pass
  move/vacancy algebra and pair order for every geometry, bound
  sentinels, the fastpath gate, copy_region edges, single-event
  contract. findings applied: the growth panic was REACHABLE in the
  turn-refused regime (u8 probe: cap 256, len 254, free holes, panic) —
  now a slide to the nearest hole (len < ceiling always leaves one);
  borrow test's second half passed for the wrong reason (the 5/6 turn
  fired at len 67 — pinned under the line now); a dead near-bank guard;
  the flip's streak-anchor wrap; stale 7/8/3/4 doc numbers throughout;
  bench census's ≥8 split relabeled a size-class heuristic.
- deferred: size sweep across the 2^20 turn-line boundary (separates
  the 5/6 ceiling's runs-read regression from threshold luck);
  multi-word banking (closes the residual runs insert tax, re-digs
  deserts); blockmap's ladder port (still fetch-gather, distance
  semantics — no Rebalance events reach bimap); streak window after a
  turn halves (anchor ×2, FREE_STRIDE unscaled); BORROW_MIN/streak-boost
  tuning; skipset/bench surface for the new event mix.

## 2026-10-21 — SkipSet vs BTreeSet at scale, STRIDE 16, the maskless tier store

- landed: the skipset tier store is MASKLESS — empty slots BORROW their
  left neighbor's handle (`K::MIN` at region heads), the descent skips
  repeats as holes (real values strictly increase), `Store::holes` gates
  the per-slot repeat-check off entirely for dense stores. `Archive`
  gained `const MIN: Handle` + `Handle: Eq` (structural — repeat
  detection); Store.slots is plain `Vec<K::Handle>` (MaybeUninit dropped
  — every in-width slot initialized, `vec![K::MIN; total]` at rebuild).
  STRIDE 32→16. bench `Item` impls gained MIN consts.
- died: the tier-store occupancy Mask (the borrow scheme replaces it —
  a mask's stale None was inert, a borrow's stale value is not, and the
  two needed three new invariants to tame, subtle_bugs §26); the
  walk_window family in blockset (linear leaf scan behind insert_at —
  measured worse, see decided); the sentinel no-mask hack (0xFF bytes,
  unsound on interior empty windows — the borrow scheme is its sound
  successor).
- decided: STRIDE=16 over 32/8/4 (sweep at 1k/10k/1M/100M: lookup floor
  sits at 16 cache-resident — 32 LOSES to the plain bsearch at 1k, 30
  vs 23 — DRAM-scale wash; upkeep tax grows as stride shrinks, 4 pays
  +27–34%; root scan must stay ≤ stride). borrow scheme over the mask
  (−16% dense lookup 1k append, ~+3–5% holed 10k, upkeep + DRAM wash —
  the mask stays sound-for-free, the borrow needs the §26 discipline).
  bsearch over the walk for insert_at's window (a linear scan with
  mask-cursor stepping, two-way Less-first compare, and a 2-word
  occupancy prologue still measured −6ns/insert at 10k vs branchless
  cmov probes; the walk's hole-spot idea deferred as a bsearch
  post-check — Future Work).
- measured (u64, best, machine-day 2026-10-21 — background miri load on
  some runs, paired A/B where noted; stride-16 mask binary = baseline):

  | random lookup | 10k | 1M | 100M |
  |---|---|---|---|
  | mask | 55.0 | 155.0* | 444* |
  | borrow | 56.5 | 148.0* | — |
  | BTreeSet lookup (random-built) | 38.9 | 626 | 709* |
  | raw BlockSet | 66–69 | 149–164 | 794–886 |

  | borrow scheme vs mask (paired A/B) | dense 1k | holed 10k |
  |---|---|---|
  | lookup | 21.1 → 17.8 (holes-gate) | ~+3–5% |
  | insert full path | parity | parity (walk runs excluded) |
  | maintain Δ | +7.2 vs +7.8 | wash |

  100M head-to-head (stride 16/32 era): SkipSet lookup 444 vs BTree 709
  (sorted-build; 626 random-built) vs raw BlockSet 886; seqread parity
  1.6–1.7. BTreeSet build order: sorted-build seqread 2.1 but
  random-built 19.4 ns/elem (leaf allocation order = insert order — 9×
  scatter penalty); random build IMPROVES lookup 13% (69% fill vs 50%,
  one level shallower). the skipset A/B phases ladder caught a +10%
  "maint regression" that cross-run cells invented — paired medians
  killed it (methodology note: never trust unpaired cells on a loaded
  machine).
- fixed during the session (all subtle_bugs §26): descent head-seed
  (prev from slot lo−1 — inherited lo can name a borrowed grid origin;
  and K::MIN at lo==0 — review agent found the Desc variant: an empty
  window-0 MIN borrow compares Greater mirrored, pins bhi to 0, every
  probe dead; minted by front bank + turn doubling + z-gate flip);
  set_cascade (maintenance writes rewrite the ==old borrow run — a
  hole's content depends on its left neighbor's VALUE, position
  intervals don't cover it — this was the ≥20k corruption root cause);
  gridline re-lift on tail pushes (tightness only — landed in the same
  batch and initially mis-credited; single-tier `tier0 > 0` guard after
  a small-cap underflow). correctness gauntlet: 47 unit tests, order
  checks 20k–1M clean, miri-clean, MIN-contract asserts added to every
  serialize cmp family.
- session-end review (fresh agent): one conditional BUG (the lo==0 Desc
  head — applied: prev seeds K::MIN, widening-only both directions),
  comments mis-locating the hazard (applied — cascade/head are the
  load-bearing sites, gridline demoted to tightness in code + docs),
  vestigial SAFETY comments on plain Vec reads (removed), MIN contract
  untested (asserts added). verified: repeat-detection soundness both
  directions, lifted fast path, apply_slide coverage, holes flag,
  bounds exactness, every Archive::MIN byte-wise.
- deferred: insert_at gap-first-hole spot pick (bsearch post-check,
  unbenched); a clean-machine re-run of the contested 1M/100M cells;
  skipset removal; the 5/6 ceiling threshold-luck sweep carried from
  2026-10-20.

## 2026-10-21 (b) — ceiling bank escape, the spot hint, and the arena plan

the session opened the arena final form (many small blocks over one index). this entry
records the landed pieces AND the multi-session plan; later sessions append below it.

- the plan (order fixed; each phase is a session-scale chunk):
  - phase 1 — deltas into the block: `Deltas` becomes a `BlockSet`/`BlockMap` field
    (`deltas(&mut self)` accessor), the consumer-supplied `&mut Deltas` parameter dies
    on every tier, `iter` drains (exhaust = auto-clear; partial consumption still trips
    the pending assert), and the event payloads (Gather's slide Vec, Rebalance's region
    Mask) become reused scratch — the per-bank/per-rebalance allocs die. the translator
    snapshot STAYS (era-stability for events held across mutations — bimap's
    pending-on-Err).
  - phase 2 — runs: banked holes placed by run direction (centered default, a signed
    Σ(pos−prev) streak sum biases), hot point migrates toward a block edge.
  - phase 3 — the arena: Vec of ≤256-slot BlockSets (A=u8), `BTreeMap<K, AA>` index v0,
    boundary-spawn split trick (append/prepend splits carry NO renames — k never
    landed), mid-gap splits and cross-block bumps share ONE closed-form event: a
    position predicate + the two translator snapshots. split trigger v0 = len == 256
    (cap-ceiling splits deferred until measured). arena-driven reversal is
    load-bearing: the z-gate cannot fire post-split (lifetime pos_sum after a dense
    128-prefill needs len > 255 — unreachable under the 256 ceiling).
  - phase 4 — presumed-SE reversal test (drops `pos_sq_sum`): measurement-GATED — the
    uniform-null z test fires on diluted front traffic (~10% front hits + 90% uniform
    reads ~5σ) that the observed-variance gate correctly holds; structural, not
    tunable away.
- landed (blockset): the len/2 bank gate's ceiling escape (`cap >= max_capacity`
  allows the long fetch — the gate's "mint instead" premise is void at the name
  ceiling); the stranded-holes ceiling phase collapsed from ~31 same-length slides to
  one bank pass (~7050 → <1000 moved, `ceiling_banks_stranded_holes` bounds it). the
  streak-hinted spot search `spot_near` (blockset.rs): bounded walk 2·FREE_STRIDE from
  `last_insert` on streak > 0, returns bsearch's canonical boundary, falls back on any
  miss; agreement asserted (`spot_near_agrees_with_bsearch`, both directions, holed);
  the staleness diagnostic answered banking-does-NOT-break-streaks
  (`streak_anchor_staleness`). new bench `mid_band` (dense 128 prefill + desc/asc
  nested bands) — keep or drop at session end.
- measured (same machine-day; word-size and small-block cells ran in a throwaway
  agent worktree — w16/w32 feature-gated experiment, NOT merged):

  word-size sweep (BlockSet insert/lookup, ns medians; u128 previously measured worse):

  | cell | u16 | u32 | u64 |
  |---|---|---|---|
  | random insert 1M | 203.3 | 174.5 | 169.5 |
  | runs insert 1M | 130.1 | 116.8 | 109.9 |
  | random lookup 1M | 197.5 | 161.6 | 156.0 |
  | random insert 1k (cache-resident) | 48.2 | 44.0 | 48.0 |
  | runs insert 1k (cache-resident) | 65.2 | 52.8 | 63.3 |

  small-block (u8 addrs, cap 256; MIDPOINT 128 → turn-refused past cap 128):

  | geometry | asc | desc | desc pre-flipped |
  |---|---|---|---|
  | fresh fill 256 | 9.2 | 17.2 | — |
  | 128-prefill, run at tail / below-min | 9.0 | 62.5 | 9.3 |
  | 128-prefill, inside run | 34.2 | 72.0 | 34.0 |
  | 200-prefill, below-min | 8.9 | 70.1 | 9.8 |

  spot_near A/B (10k, u64 keys, gate off → on): runs insert 84.5 → 59.5 (−30%);
  random insert 89.5 → 87.6 (wash); mid-band at 256-cap 87.8/93.5 → 82.0/86.8 —
  the hint's win scales with the bsearch it kills (sparse-mask probes at DRAM scale).
  mid-band direction is SYMMETRIC (desc 87.8 vs asc 93.5, 0 flips — the z-gate never
  fires mid-band); the desc penalty is edge-only.
- decided:
  - u64 mask words stay (rejected u16 — loses everywhere, ~10× bank events dig
    deserts; rejected u32 — wins only cache-resident, pays 2–4× loads on sparse-zone
    probes at DRAM scale). word size becomes a generic through mask consumers later.
  - split feedback = the position predicate + to_translator (rejected: explicit
    (BA,BA) pair Vecs — strictly dominated; rejected: arithmetic name offset —
    rotation-fragile, two code paths). at-split blocks are DENSE (len == cap == 256 ⇒
    mask full) so the moved run is contiguous positions — that is what makes the
    closed form possible.
  - index v0 = BTreeMap<K, AA> (rejected: native tiered index now — the K: Ord cost is
    accepted for v0; the eventual index compares min-clones).
  - fork 2 remainder OPEN: (AA,BA) pair vs packed opaque addr as the user-facing form.
- deferred: the (AA,BA) packing representation; cap-ceiling split trigger (vs len ==
  256 — the ceiling fix removed the worst of the turn-refused grind, re-measure
  first); blockmap's ladder port inherits the ceiling escape when it happens.

## 2026-10-22 — runs banking, the ceiling valve, and the arena lands

phase 2 (runs) and phase 3 (the arena) landed on the 10-21 plan. the session's
verdicts are mostly deletion records — three mechanisms built, measured, and
removed.

- landed (blockset): the centered bank landing (`bank_landing` — any slot in
  the compacted hole range is order-legal; k takes the center, slack-clamped);
  the anchor-follow (`last_insert` tracks the previous element through
  slide_hole/compact_gather/move_slot/grow_rotate/reverse — 0 stale, verified
  by an honest-holder diagnostic that APPLIES name swaps, the old one measured
  name-staleness artifacts); the ceiling valve (`rebalance_block` — the
  preemptive turn's replacement when doubling is refused: whole-block
  minimal-stride lattice fitting len below the name ceiling, tail extended to
  the tight extent; the mid-run O(n)-per-insert grind is gone); `cleave` (raw
  slot/mask copy, `Translator::shifted(at)` = offset − at so MOVED NAMES
  SURVIVE and the split event is "position ≥ split_pos ⇒ new block id", heap
  rebuilt compact on both halves, signals re-seeded, Desc halves flipped Asc
  — see subtle_bugs §30/§31); `min_key`, `rebuild_heap`, `seed_uniform_signals`.
- landed (arenaset — the block-of-blocks tier): `ArenaSet` — Vec of u8-name
  BlockSets under `BTreeMap<min-clone, id>` (the index order IS the block
  order); floor-entry routing; boundary spawns (fresh block takes k alone —
  no renames ever for sequential streams); 50/50 cleaves with the one-field
  split event; arena-owned below-min flips (split-born halves have a dead
  z-gate); dup-vs-capacity discrimination by lookup (§28); 8 tests incl. a
  full holder-mirror torture (every name verified after every insert) and a
  String-key heap-rebuild test.
- died: the run-direction signal (raw Σ(pos−prev) is stale across a
  streak-continuing direction change AND self-reinforces through its own
  landing; EMA-damped still lost to plain centering — do not revive without a
  new workload); the len/8 hot-bank gate and the want-decoupling (measured
  worse, reverted — the mid-run is the adversarial case, not the tuning
  target); the ladder-insert cleave (z-gate flips the newborn mid-cleave,
  re-archiving churns the heap — replaced by the raw copy).
- decided: keep anchor-follow (correct invariant; costs runs movement via
  the streak→want coupling — the ladder's streak tuning is a sweep-worthy
  task of its own); centering over direction-led (three-arm A/B, both bands);
  raw-copy cleave over insert-cleave (names survive, event collapses);
  `from_tr` snapshots stay in Deltas (era stability for held events).
- measured (moved/ins, deterministic, this machine):
  | cell | pre-phase-2 | landed |
  |---|---|---|
  | runs @10k | 7.19 | 8.57 (the anchor-follow cost, accepted) |
  | desc band iso | 5.97 | 5.78 |
  | asc band iso | 6.66 | 5.31 |
  | runs time @10k | 59.6 | 52.1 (−13%) |
  | random @10k | — | bit-identical movement across all phases |
  bank-landing three-arm (bands, spot-adjacent / centered / direction-led):
  5.97/5.23/6.02 desc, 6.66/5.73/5.83 asc. ceiling valve: 200 live/256 →
  stride 5, cap 251, follow-ups amortize (1,4,15,1 moved). anchor-follow
  streak census: 5 resets/256 band (transition + ceiling-jumps structural;
  the Δ7 slack off-by-one fixed by budgeting from `open`).
- NEXT TASK (the user's, notes/devlog.md "Reversed translator math"): block
  splitting with the corrected translator math — offset is ALWAYS recalculated
  on rotate/split via the complete formula
  `new.a2p(old.p2a(at)) = 0 ⇒ new_offset = −((old_offset + at) rl old_rot rr new_rot)`
  (valid for arbitrary split points and both halves; `len < MIDPOINT` becomes
  the only in-place rotation constraint, replacing every offset restriction);
  the midpoint-split-then-rotate (the would-be-wrapped region lives in the
  empty half); the derotation in small-width examples is the mod-width
  identity, not a mechanism. this supersedes the rebalance-block valve as the
  ceiling graduation and likely replaces cleave's Desc-flip guard.
- deferred: multi-word banking (elevated: with honest streaks the one-word
  borrow bound is the binding constraint — the fork re-banks every ~7
  inserts re-dragging its own stranded holes); the streak/ladder retune
  sweep; (AA,BA) packing; arena bench; arena removal/merges; the above-max
  symmetric eager flip; skipset-tier-per-arena-block.

## 2026-10-23 — the gather rewrite: subdivide, strided banks, the mean-rebalance, and the fork measured twice

a full day on blockset's gather. the arc: subdivide → greedy banks → strided
(sentinel) landings → want tuning (dist/6, log2) → run-preferring fetch → the
block-mean water-level → the fork restored. every single-op config was built,
measured, and most were deleted; the final ladder is the old topology with
every rung upgraded.

- landed: SUBDIVIDE (the clear run between k's live neighbors is order-legal;
  interior gaps place k mid-gap, tail gaps at gap_lo — the append edge stays
  solid; zero movement wherever the boundary gap has room); the reversal
  trigger swap (`pos_sum < len·ilog2(len)` — one signal, pos_sq_sum/REVERSE_Z2
  commented out at all six sites); the STRIDED bank (hole i lands at
  `open ± (i + i/15)`, the run's head `drag = (w−1)/15` elements ride in as
  sentinels so ≥1 live sits per 16 across the banked range; encoded in the
  Gather buf's landings, no new event shape — blockmap's compact ported in
  lockstep, pairs move via mem::replace); the RUN-PREFERRING fetch (once a run
  member is taken the walk stops at the next singleton — singletons are the
  floor of at-average regions at the ~1/2 cycle mean; leading singletons ride
  along because compact's segment math needs a solid span); the MEAN
  water-level (`rebalance_region` reborn: two scan arms — above_mean
  (budget-capped, live·cap > len·span in u128) and unworkable (live·6 ≥ span·5,
  uncapped); tail-grow serves the unworkable arm only; slack-0 and
  ceiling-blocked stops slide from the nearest hole); the mint arm (solid mask
  mints `1 + (cap−spot)/FREE_STRIDE`, clamped to the name ceiling).
- died: the word-classified NEAR/BORROW/FORK ladder (HIGH/BORROW_MIN lines —
  the run-preferring fetch + near-bank replace them); the ceiling valve
  `rebalance_block` (commented out — the mint/bank ladder handles the refused
  turn); want = log2(dist) (amortizes at dist/log2(dist) — unbounded; killed
  the runs win at 1M 68→95 and blew the ceiling guard 1759); want = dist/6 as
  the ONLY far arm (bank-only random 572-730); the bank-only and
  rebalance-only ladders (each measured, each lost the other workload).
- decided: the fork is load-bearing (hot = bank, cold = water-level — see
  subtle_bugs §33, measured twice); NEAR holes (≤ 64) bank unconditionally
  (gating them on the streak regressed random@10k 90→109); the rebalance stop
  needs the workability FLOOR (a mean-only stop lets lattice stride exceed
  the slide reach above 5/6 — subtle_bugs §32); the budget carries +64 (the
  scan is word-granular); blockmap keeps the old reversal line (divergence
  documented at should_reverse, port deferred).
- measured (sorted_insert, medians, u32 addrs; machine-day noise ±5-10% on
  1k rows — one append row read 14.2 with SortedSet shifted equally):

  | config | random@1M | runs@1M | runs@10k |
  |---|---|---|---|
  | old ladder (10-22) | 274.6 complete | 92.4 | 53.3 |
  | greedy bank dist/6 | 730 partial | 70.8 | 42.0 |
  | + strided sentinels | 571.7 partial | 68.0 | ~42 |
  | log2 want | 553 partial | 95.3 | 41.7 |
  | mean-rebalance only | 292.8 partial | 5419 partial 557k | 160.4→241.9 |
  | fork + near-bank (final) | **287.8 complete** | **71.3 complete** | 48.5 |

  final ladder vs the old ladder: append 9.3/9.1/12.9 (was 9.7/9.5/13.4),
  prepend 10.0/9.2/11.6 (11.1/9.6/12.2), random 45.9/89.7/287.8
  (43.1/90.7/274.6 — parity), runs 40.1/48.5/71.3 (42.8/53.3/92.4 — −23% at
  1M). vs BTreeSet at 1M: append 4.4× faster (12.9 vs 57.2), prepend 1.8×,
  random 1.4× slower (287.8 vs 205.6), runs 1.8× slower (71.3 vs 40.4) —
  the skip-list tier's mandate. miri clean on the blockset module (1098s);
  fresh review agent: no correctness bugs, 8 findings applied (empty-fetch
  fallbacks → §25 debug_asserts, blockmap near-gate off-by-one aligned,
  z-gate test renamed to what it pins, §30 closure re-derived, left-sentinel
  mirror test added). POST-REVIEW, the skipset bench's own order assert
  caught a LATENT skipset bug older than the session: apply_slide's
  changed-window range used ceil(lo/stride), skipping any interval's
  own window when it starts mid-window — a subdivide point event then
  left a tier0 window-first stale-high, the descent excluded an
  insert's true boundary, and insert_at broke order (maintained
  runs@10k, insert 13; subtle_bugs §34, fixed to floor,
  regression-pinned).
  regression-pinned). the skipset bench re-run (1M, medians): the
  tiers flip the lookup story — random lookup SkipSet 119.8 vs
  BTreeSet 179.9 (1.5× faster) and flat BlockSet 169.3 (−29%);
  append lookup 117.8 vs 194.3; inserts stay BTree's (random 114.4
  vs 170.3 base / 182.2 maintained); the wrapper costs ~0.8ns over
  the base, upkeep+descent ~12ns on random. at 10k (cache-resident)
  the tiers still lose lookups to BTree (51.3 vs 38.2) — the tier
  win is DRAM-scale, as the L2 build gate always predicted.
- deferred: blockmap's ilog2 trigger port + its ladder port to the new fork;
  test gaps (slack-0 budget-short fallback, rebalance ceiling-bail, mint
  clamped by room); subdivide's two extra mask scans at DRAM scale
  (unbenched); the loosened trigger vs alternating front streams without
  tail filler (ungated, untested); the skipset bench re-run under the new
  ladder.

## 2026-10-23 (b) — the fn-ptr dies for serialization, and SkipSet::update lands

two deliverables: a serializable translator and the addr-keyed replace. the
fn-ptr question started as a microbench curiosity and died on a security
argument — a memmapped block would serialize its translator verbatim, and a
stored code address is a hijack primitive. the branch shape measured faster
anyway, so the dispatch cost nothing to kill.

- landed: the translator's inline `reversed` branch (rotation+offset
  unconditional, `#[inline(always)]` — fn-ptr fields + their four fns
  deleted); `SkipSet::update(addr, new) -> Result<(A, K), UpdateErr<K>>` —
  Ok = (new addr, the OLD key); FAST PATH: `new` strictly inside `addr`'s
  immediate key bracket (direction-aware) ⇒ `BlockSet::replace_key` swaps
  the handle in place (name preserved, no deltas event, one
  apply_slide(p, p) window-chain re-read); SLOW PATH: dup pre-check (makes
  Err atomic) → remove → tier fix → `hint_window` bracket walk (±2·STRIDE
  from the vacated slot, monotone-sound, head/tail exhaustion = valid
  bounds) bounding insert_at, descent/beyond_tail fallback; same-key
  update = Ok((addr, new)) no-op; `BlockSet::prev_live_pos` (the hint
  walk's down-side stepper); `remove` now DECREMENTS pos_sum (the signal
  tracks current elements — makes the flip trigger reachable in
  update-heavy streams); benches `translator.rs` + `update.rs`; tests
  update_torture / update_descending / update_in_place /
  update_no_tiers_far (miri-clean, ~5 min the update pair).
- died: the fn-ptr dispatch (serialization hazard + measured slower — the
  header's BTB-prediction "~1 cycle" claim was wrong: the indirect call
  costs load+call+ret ≈ 4 cycles); the bench's cross-pattern far workload
  (moved to `diag` — it measures the base ladder's one-sided-desert
  pathology, not update).
- decided: the strict bracket IS the dup check (only the immediate
  neighbors can equal `new`, Equal fails both sides — no separate find;
  subtle_bugs §35); Err atomicity via pre-check, never rollback (a
  rollback re-enters the hazard it escapes — §35); "nothing moved" still
  owes the window-chain re-read (copied-value staleness ≠ position
  staleness — §35); Ok carries (addr, old key) — the slow path can change
  the name, so the addr must return.
- measured (update.rs, ns/op best-of-10, even-key append prefill, tiers
  built; vs BTreeSet remove+insert; machine-day noise ±5-10%):

  | workload | 1k | 10k | 100k |
  |---|---|---|---|
  | near (fast path) | 15.6 vs 35.1 | 17.6 vs 69.6 | 19.7 vs 74.9 |
  | far (slow path + holder retarget) | 32.5 vs 28.6 | 39.9 vs 97.8 | 60.9 vs 128.5 |

  the fast path holds ~4× at DRAM scale; BTreeSet edges far at 1k
  (cache-resident). translator dispatch (benches/translator.rs, shapes
  asserted equal per cell): fn-ptr 0.99–1.58 ns/call vs branch 0.15–0.25
  — the microbench flatters the branch (unrolled ILP), true gap in situ
  ~3-4 cycles; both uniform across rot 0/13 × fwd/rev.
- deferred: window-bounded dup pre-check (slow path pays a full descent
  even when the hint bounds a nearby window — report-only, unbenched, in
  CLAUDE.md Future Work); the update-top flip branch has no deterministic
  test (pos_sum decrements make it reachable but not provably); the
  tier-less update never flips (insert auto-flips there — divergence
  documented at update's doc). fresh review agent: one confirmed panic
  (the no-tiers far fallback hit window_of's "tiers built" unreachable —
  fixed to the full-block window, repro test update_no_tiers_far pinned)
  + the test gap that let it through (no-tiers far/far-dup ops added to
  update_torture); everything else checked out guarded (hint_window Desc
  mirroring, bracket dup detection, Err atomicity, §22 flip/record
  ordering, pos_sum wrap analysis, the bench's holder retarget).

## 2026-10-27 — block substrate: crate + archive v2 + Stacks + blockset port + freelist
manager-mode session: tasks delegated to fresh agents, each diff reviewed before the next launched.

- landed: new `block` crate — `Block<P>` dual-front allocator (relptr'd
dynamical trailing data, no const generics; zero-record invariant), `Archive<P>`/`RelOrd<P>`
+ `Allocator` (write seam, snapshot/rewind) + `Heap` (read seam), `Stack` (snapshot-unit
LIFO full erase) + threaded `Stacks` over kanal (owner actor, leases, LRU eviction),
heap freelist (in-hole stamps), overhead bench w/ channel A/B; new `block-derive`
crate (mirror handle structs, layout canaries); doa `BlockSet` ported onto the block
substrate, old blockset frozen as `blockset_v1` for skipset/arenaset; workspace
CLAUDE/notes restructure (workflow+style up a level, session history workspace-wide,
block devlog split, block/notes/subtle_bugs seeded).
- died: doa's `legacy-v1` feature gate (didn't compile — replaced by the frozen `blockset_v1` copy).
`Archive::drop` (renamed `release`, clippy trait-name + write-seam symmetry). Heap-as-`&mut`
(on the cmp path — poisons lookups; read view won). Stacks single-threaded v0 (user call:
no point).
- decided: relptr dynamic data over fixed-only+const generics; zero-record invariant over
uninit bytes (user call: serialized stale bytes are a leak); RelOrd FIELD-WISE over view
construction (user call: inlining + wasted field builds); kanal over std mpsc (8.6×);
Stacks full actor thread design from day one; skipset/arenaset stay v1 via frozen copy
(porting blocked on v2 MIN + materialize); blockset cleave = verbatim tail heap copy
(no materialize to rebase nested handles); T7 agent output-cap deaths → task sliced into
5 sub-delegations.
- measured:
| cell (ns)                      | std mpsc | kanal  |
|--------------------------------|----------|--------|
| channel RTT floor (echo)       | ~5600    | ~650   |
| Stacks::push                   | ~5800    | ~700   |
| read-closure 1-item            | ~5600    | ~710   |
| lease grant                    | ~5800    | ~800   |
| lease amortized (held view)    | —        | 39     |
| eviction alternating (b=1)     | ~7100    | ~1750  |
| flush amortized /push          | 7–8      | ~4.4   |
| 4-thread push aggregate/item   | ~1190    | ~310   |
| bounded(1) vs unbounded replies|          | noise  |
conditions: this box, 3s wall cells, med-of-5 reps; raw storage
(archive/push_borrowed) 3–6ns/item either way. suites final: block 65+9, block-derive 8,
doa 135, all-targets clean, miri clean (stacks integration needs -Zmiri-disable-isolation).
- deferred: Archive view/materialize + MIN const + LE policy; block compaction
(handle-fixup protocol); Stacks persistent-reply/fire-and-forget then per-block-latch
redesign; blockset remove-release wiring + with_byte_cap + DEFAULT_SLOTS retune; doa doc/
stale session_history comment refs (rid a code session + skeletonize refresh).

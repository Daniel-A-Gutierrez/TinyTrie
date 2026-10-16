# session history — doa

the crate's working history: the problem a session worked on, the approaches tried, what
worked and what didn't, and why — plus the numbers that chose between them.
CLAUDE.md carries the living map (concepts, contracts, current state, future work); anything
temporal belongs here, not there.
the devlog is the user's own writing — sessions never write to it or point readers there.
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

# block — subtle bugs, and the rules they left behind

Solved nuances of the crate, each with the trap, why it was easy to miss, and
the rule. The point is not history — it's that each of these generalizes to a
rule, and the rule now lives in the code.

Cross-referenced from `CLAUDE.md`; the operational summary of the rules is
there, the reasoning is here.

---

## 1. Realloc's extension is garbage

**The trap.** `realloc` hands back an unchanged prefix followed by
uninitialized bytes. The zero-record invariant says a serialized record holds
only live data, hole bookkeeping, or zero — but a naive `grow` leaves the new
tail holding whatever the allocator had there:

```
[ live bytes ][ free space ]  --grow-->  [ live bytes ][ UNINIT GARBAGE ]
                                                        ^ reaches the file
```

`stack_bytes` premised its zero-reading on the tail being wiped; one grow
without a memset poisons every record that reads past it.

**The rule: `grow` zeroes the new tail, every time.** realloc's extension is
uninitialized by contract — treat it as garbage on arrival, not as "probably
zero".

---

## 2. HANDLE LAYOUT LAW: implicit padding memcpy's uninit into records

**The trap.** `Block::push` memcpy's the handle verbatim into a record. A
repr(Rust) handle's implicit padding bytes are uninitialized, so every record
of such a type carries uninit bytes — a zero-record violation with no user
bytes deleted, invisible in release runs until a load reads them back.

**Why the naive fixes fail:** const-array-length pad assoc types, and the
other variant tried, are both rejected on this nightly (trait consts cannot be
array lengths); the inline-const canary had to be probed too — plain const
items and array lengths cannot use generic projections, which is the same
nightly limitation that shapes the derive.

**The rule:** handles are POD with EXPLICIT padding ONLY.
- per-width pad assoc types on `Ptr` (`TailPad2/3`),
- `#[repr(C)]` on EVERY handle — repr(Rust) reordered the explicit pads into
  wrong record positions once, so it is load-bearing, not stylistic,
- a zero-size `PhantomData<[P; 0]>` marker — a width-sized marker re-poisoned
  composite tails,
- the derive emits an inline-const layout canary (evaluates per-
  monomorphization): a dirty width is a compile error, not a dirty record.

---

## 3. Snapshot must clone-capture (heap + holes)

**Historical note:** the snapshot/rewind mechanism was replaced 2026-10-28
by the unwind protocol (§13) — this entry stays as the record of what the
old design had to get right.

**The trap.** An archive that fails mid-build calls `rewind` to leave the heap
unchanged. If the snapshot does not OWN a copy of the hole list, a free that
happened post-snapshot inserts a hole that pruning cannot restore — and holes
carry stamps, which the archive's writes and the rewind wipe destroy:

```
snapshot at heap 100            free(x) at hole H      rewind
[ data ][ H stamped ]    →      H's stamp erased       stamp gone —
                                                               H unusable
```

**The rule:** snapshot clone-captures both the heap length and the hole list;
rewind wipes the rolled-back span (else failed-archive bytes survive below the
new bottom — a zero-record violation) and RE-STAMPS every restored hole
(archive writes + the rewind wipe destroy stamps; a consumed hole must come
back correctly stamped or the freelist silently shrinks).

---

## 4. Sub-8 holes carry no stamp

**The trap.** An 8-byte stamp `[MAGIC|len]` needs 8 bytes; a freed region
smaller than that cannot be stamped. From_record's load path reconstructs
holes by a MAGIC scan — an unstamped hole is therefore legitimately invisible
at load.

**The rule:** the stamp is a persistence boundary, not a correctness
requirement — sub-8 holes are runtime-only, and from_record's scan DROPS them.
But a pop boundary straddling a hole must re-stamp the clipped survivor
(else the surviving piece is invisible at load and its bytes look live/leaked);
if the clipped survivor is < 8, it drops to an untracked gap and the runtime
freelist must not claim it is loadable.

---

## 5. Slack-below splits are impossible (for align ≤ MAX_ALIGN/2)

**The trap.** The split arithmetic assumed a hole could be split with slack on
the low side — `align_up(span, a)` can undershoot the request. For any `a ≤
MAX_ALIGN/2`, `align_up(span, a) − span < a` BY CONSTRUCTION, so a slack-below
piece is always smaller than the alignment itself: never a usable hole.

**The rule:** split arithmetic computes `align_up − span` expecting overflow
at the edges — it must SATURATE. (One overflow panic was caught here; the
impossibility above is why the case is provably dead, not merely rare.)

---

## 6. Stack erasure: the unit, the pop, the birth record, the final ack

**The trap(s).** Four separate ways deleted stack items leak or read corrupt:

- the unit is `(heap-snapshot u32, Handle)` serialized IN the record — pop
  needs the snapshot to roll the heap back to; storing it anywhere else
  (`from_record` gets no extra state) breaks load.
- pop is `pop_heap(DELTA to snapshot)` + wipe, NOT a pop-to-absolute-position
  call — a to-position call wipes bytes above the snapshot that belong to
  newer units.
- an unflushed NEW block reading back as a zeroed-cap header reads corrupt —
  the birth record writes immediately (miri-caught reopen race).
- the last-handle drop BLOCKS on ack-after-flush — an async-exit flush let a
  reopen read nondeterministic bytes (miri-caught).

**The rule:** every erase path is wipe-the-bytes-then-roll-the-offset, and the
record format must never depend on flush timing — anything a reopen could read
must already be on disk before the last handle can drop.

---

## 7. `free` name collision: capacity getter vs release

**The trap.** `free` read as "bytes free" on one call site and "release this
handle" on another — silently compiling at both.

**The rule:** capacity is `free_bytes()`; releases are `free(h)`. The
namespace is owned by the release operation, which is the dangerous one.

---

## 8. kanal oneshots and the handle-clone model

**Historical note:** the channel transport was dissolved 2026-10-27 (the
per-block-RwLock rewrite) — this entry stays as the transport A/B record the
bench's echo cells still pin.

**The trap.** Assumed a bounded(1) response channel was safer than an
unbounded oneshot; measured, both are within noise at the ~650ns RTT floor
(the bench cells pin it) — the bound buys nothing.

**The rule:** the shared request channel is UNBOUNDED; per-request oneshots
are bounded(1) for free (they die with the request). Sender/Receiver are
Send+Sync (MPMC) — that property is what keeps the handle-clone model sound:
cloned handles share the owner through concurrent sends, so nothing about a
handle's lifetime cares which thread holds it.

---

## 9. StrHandle's inline marker rides on heap_bounds

**The trap.** `h.0 == 0` as the "inline" flag looks ambiguous — a heap handle
could have offset 0 pointing at the heap start. It cannot: empty vecs
legitimately use `H(0,0) + len 0`, and any heap allocation has len > 0.

**The rule:** `heap_bounds` requires `h1 ≤ h0` (an allocation's end-offset ≤
its start-offset reading back-to-front) — inline handles reaching
`Heap::bytes` panic by construction, so `h.0 == 0` is unambiguous ONLY under
that invariant. Don't loosen heap_bounds without re-deriving StrHandle's
marker.

---

## 10. Coalescing and double-free detection

**The trap.** Two freed regions that touch are merged; if the merged entry
kept both stamps, alloc would see two holes that no longer exist. And
double-free detection that checks "is this exact hole in the list" breaks the
moment a free's hole was absorbed by a neighbor.

**The rule:** coalescing keeps holes PAIRWISE NON-TOUCHING; absorbed holes'
stamps are erased and exactly ONE stamp lives at the merged start.
Double-free detection is SPAN-COVERAGE (merged entries absorb exact
identities, so a re-free of any absorbed span is caught), and zero-length
frees skip the check — a zero-length span "covers" any hole that contains
its offset, which would be a false positive.
---

## 11. The fresh-record Cap race (concurrent spills)

**The trap.** The serial spill rule was "Err in the record you just appended
= Cap" — only an item exceeding a whole record fails in a fresh one. Ported
to the lock design by remembering the appended record's id, it misfired:
between a producer's spill and its retry, ANOTHER producer can fill that
record, and the retry's full-block Err then matched "my record" → a spurious
`Cap`, silently rejecting a pushable item (the bench's threads2 cell caught
it live: `Cap(CapErr { free: 16 })` on 32-byte strings).

**Why it was easy to miss.** Every single-threaded test passes — the race
needs two producers around one record transition, and "an Err is an Err"
looked sufficient because LIFO made the retry target look predictable.

**The rule.** The oversize signal is BLOCK STATE under the write lock, not
record ownership: Err + `stack.is_empty()` ⇒ Cap (an empty block only
rejects an item bigger than a whole record); any other Err is a full block
⇒ spill, and spill re-checks `top` under the shared lock so racing producers
never double-append either.

---

## 12. In-band bookkeeping dies with the span that names it

**The trap(s).** The v2 freelist stores its nodes inside the very bytes they
describe, so any op that wipes or hands out a span must remember the node
living in it. Three ways this bit during the rewrite, each caught by a test:

- `pop_heap` wiped the rolled region BEFORE collecting the list — a hole
  straddling the new bottom had its node inside the wiped span, so the walk
  read a zeroed node, the hole died, and the clipped survivor vanished
  (`pop_straddle_clip_rethreads`).
- `alloc`'s hole-take served the region without zeroing it — the taken
  hole's node bytes rode into the caller's record as stale bookkeeping, a
  zero-record violation that only byte-level assertions caught
  (`alloc_reuses_hole_exactly`).
- `rewind` restored the snapshot's holes but never cleared the DROPPED
  holes' nodes — bytes the rollback turned dead stayed nonzero above the
  restored bottom (`rollback_after_extra_free`, the v1 §3 lesson resurfacing
  in a new shape; rewind itself died with §13, but the class — dropped
  bookkeeping reaching no wipe of its own — is why every span transition
  must clear what it drops).

**The rule:** whenever the freelist's storage is the heap bytes themselves,
every span transition — wipe, take, rollback — must either rewrite the nodes
it keeps (via `sync_holes`) or zero the nodes it drops. Collect state BEFORE
wiping; zero BEFORE handing out. (With the lazy cache the collect is the
materialize; the rule is enforced by materialize-first, not vigilance.)

**And the neighbor rule** (same session): the double-free coverage predicate
is `hole_off >= off && hole_off + len <= off + hole_len` — off+len ≤ o+l is
the INVERSE test and false-panics on merely adjacent holes
(`free_coalesces_hole_above`).

---

## 13. The unwind protocol replaces the snapshot (2026-10-28, user design)

**The insight that dissolved a mechanism.** v2-v3 carried `Snapshot`/`rewind`
for archive atomicity — clone-capturing the hole state (§3) and, once the
list moved in-band, walking it into a Vec at every entry, success included.
But every single `alloc` is atomic by construction and hands back an `H`
sufficient to free exactly what it took — so a failed multi-alloc archive
can unwind itself: release the children it built (children first via
`release_slice` — the partial handle plus the write count is the undo log),
then `Allocator::rollback` rolls the front to the entry position. Success
pays no atomicity tax; failure pays the frees.

**The two supporting rules:**
- ARCHIVE IMPLS NEVER FREE on the success path — the unwind is the only
  freeer inside archive. This makes the front rollback sound (everything
  below the entry front is provably the failed archive's own) and keeps
  `free`'s double-free panic a tripwire rather than an unwind hazard.
- `free` JOINS holes across sub-NODE gaps (live regions are ≥ NODE, so a
  smaller gap is provably dead align slack) — without the join, unwinding a
  hole-take would leak the alignment crumbs the take consumed; with it the
  unwind is near-exact (`rollback_joins_then_clips` pins the shape).

**The trap the review caught on the way in:** Stack's handle-push Err path
shrank to the snap's ALIGNED start, stranding the snap's alignment gap on
the stack front for sub-4-aligned handles — the residue broke the stride
canary on the next pop (`failed_push_leaves_no_stack_residue`). Rule: undo
to the PRE-PUSH front, never to a landed item's offset.

**And the lazy cache (same session):** the hole list materializes into
`Block.holes` at first use — loads are a byte copy, the first hole-touching
op pays the one validating walk, and `sync_holes` keeps the bytes canonical
after every mutation. A corrupt head may not self-link: real holes sit
strictly past the head's own NODE bytes (off ≥ NODE + len), so the walk
rejects `off <= NODE` outright.

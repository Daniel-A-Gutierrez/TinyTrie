# block — serializable fixed-storage blocks

The problem: a database index needs blocks that serialize as raw bytes, keep
every operation O(1), never leak stale memory into the serialized form, and
still allow arbitrary user types to move in and out via handles. Every
mechanism in this crate is a consequence of taking those four constraints
seriously.

## The concepts, ranked

1. Relptr + dynamic trailing data (worldview). `Block<K>` is a 16-byte header
   plus a dynamically-sized data region in ONE allocation, named by a byte
   offset (`ptr`) from the allocation base — no const generics on data size,
   and the offset survives any move of the whole box. Growth reallocs; the
   relptr absorbs it. Pinning exists only if header/data were ever separate
   (they aren't).
2. Two pointer spaces (worldview). `S<P>` = (start byte offset, byte len),
   stack-relative; `H<P>` = (back-to-start offset, byte len), heap-relative.
   Distinct types so neither passes for the other. Both survive `grow` — the
   heap tail slides to the new back (H's offsets are back-relative, untouched),
   the stack never moves (S front-relative). Growth = pure back extension.
3. The zero-record invariant (the crate's law). A serialized record contains
   ONLY live data, hole bookkeeping, or zero: born `alloc_zeroed`, `grow`
   wipes the realloc'd tail (realloc's extension is garbage), and every
   reclaim (`stack_shrink`, `pop_heap`, `Allocator::rewind`) wipes what it
   rolls back. Deleted user bytes never reach a file — this is a security
   property, not a nicety. `alloc` memsets nothing: regions claim zeroed gap.
4. HANDLE LAYOUT LAW (sharp edge). Handles are POD with EXPLICIT padding
   only — a repr(Rust) handle's implicit padding bytes are uninitialized and
   memcpy'd verbatim by `Block::push` into the record. `StrHandle`/`VecH`
   carry per-width pad fields (`Ptr::TailPad2/3` — trait consts cannot be
   array lengths, this nightly rejects both variants), every handle is
   `#[repr(C)]` (repr(Rust) REORDERED pads into wrong record positions once —
   it's load-bearing), and the derive emits an inline-const layout canary so
   a dirty width is a COMPILE error, not a dirty record.
5. Archive v2 stores, RelOrd compares (separation + field-wise). `Archive<P>`
   has no Ord bound: `archive` allocates each field through the Allocator
   (which pads — archive computes nothing) and returns a fixed-size POD
   Handle (+ `HEAP: bool` const; + `release` for children-first recursive
   frees). Comparisons are `RelOrd`'s `rel_cmp`/`rel_cmp_h`, FIELD-WISE,
   short-circuiting — no view construction; the user rejected views for
   inlining + wasted fields. Capacity feedback = try-then-rewind:
   `Allocator::snapshot()/rewind()` makes any failed archive leave the heap
   unchanged. No preflight sizing call exists anywhere.
6. Stack = LIFO full-erasure storage. Each stack unit is
   `(heap-snapshot u32, Handle)` serialized IN the record; `pop` shrinks (wipe)
   then `pop_heap`s to the unit's snapshot (wipe) — deleted items leave no
   bytes anywhere in the file, with zero freelist machinery, because the
   snapshot rides the record and `from_record` needs no extra state. Record
   stride = 16 + cap; `RECORD` 4096 ⇒ cap 4080.
7. Stacks = the actor model over an owner thread. One owner owns file +
   resident map + LRU budget; handles are clones sharing it via kanal
   (unbounded request channel, bounded(1) per-request oneshots — kanal was
   8.6× the std mpsc floor: ~650ns vs ~5.6µs RTT; Stacks::push ~700ns ≈ floor
   + ~50ns visible owner work). Reads go two ways: `read` closures run
   OWNER-side (exclusive, panic-isolated via catch_unwind → resume on caller);
   `lease` ships an `Arc` record snapshot — the Arc IS the reader token
   (reader counting is LEASE-ONLY; closures serialize through the owner so
   eviction can't race them). Eviction: LRU, dirty + write-back, skips
   readers + MRU (deferral, never blocks). The last handle's drop BLOCKS on
   the owner's ack-after-flush — a reopen afterward is deterministic
   (miri caught the async-exit race). A new block's birth record writes
   immediately (an unflushed block must read back as a valid empty record).
8. The freelist (v1). Runtime `holes: Vec<(back_off, len)>` sorted; freeing
   wipes user bytes then stamps an 8-byte [MAGIC|len] header in the hole
   (sub-8-byte holes skip the stamp — runtime-only, the documented persistence
   boundary); `alloc` best-fits holes O(holes) with O(1) splits, fast path
   unchanged when hole-free; coalesce at free merges touching neighbors;
   rewind restores the snapshot's hole list AND re-stamps (archive writes and
   the rewind wipe destroy stamps — a consumed hole is restored correctly);
   load reconstructs holes by a MAGIC scan over the heap span. Full compaction
   is DEFERRED — moving live regions stales caller-held H's, so it needs the
   handle-fixup (deltas-style) protocol first.

## Files (lowest → highest)

- `src/lib.rs` — wiring + re-exports.
- `src/handle.rs` — `Ptr` (integer ladder + `BYTES` + TailPad assoc types),
  `S`/`H` newtypes, layout law docs.
- `src/block.rs` — `Block<P>` core: dual-front allocator, Header (repr(C,
  align 16), 16 bytes, pad-free serialized form), wipes, grow, freelist,
  `Snapshot`/rewind, `Allocator` (write+release seam), `Heap` (READ view —
  cmp paths need only &Block; the &mut-from-read mistake was caught in review).
- `src/archive.rs` — `Archive<P>`/`RelOrd<P>`, `Heap` read seam, StrHandle/
  VecH handles (pad fields per width), impls for primitives/char/String
  (8-byte BE prefix cutoff, h.0==0 = inline)/generic Vec<T>.
- `src/stack.rs` — `Stack<T>` + record save/load (`LoadErr` 8 variants,
  stride validation, header sanity; byte-for-byte loads, no field decode).
- `src/stacks.rs` — threaded `Stacks` manager (kanal; `Lease`, budgets,
  eviction, `DEFAULT_BUDGET` 8, multi-record files).
- `benches/stacks.rs` — the wall-budget overhead bench; carries the
  std-vs-kanal echo A/B (`echo-rtt-floor` vs `echo-rtt-floor-kanal`/unbounded).

## Testing

Inline `#[cfg(test)]` in every module + `tests/stacks.rs` integration.
Miri-clean throughout; file-touching tests are gated `#[cfg(not(miri))]` and
the stacks integration suite needs `MIRIFLAGS=-Zmiri-disable-isolation`
(it does real fs I/O). The zero-record invariant, pad law, hole stamping, and
pop erasure each have dedicated pin tests — when touching any of those paths,
the pins must understand why they exist.

## notes

`notes/devlog.md` (block-substrate design sections moved here from doa's),
`notes/subtle_bugs.md` (solved nuances of this crate). Session record is
workspace-level.

## Future Work

- Stacks transport: persistent per-handle response channel (request-id match)
  + fire-and-forget/batch push — bench cells exist for the echo A/B; the
  measured prize is batched/pipelined access (serial RTT is inherent).
- Stacks architecture: per-block RwLock latches + atomic last-touch/live
  counters dissolving the owner thread (read closures die; push ≈ lock +
  storage ~tens of ns). Needs the deterministic exit-flush and push
  panic-rollback preserved. Deferred after the cheap wins.
- Block compaction with a handle-fixup protocol (deltas-style move reporting).
- Archive: view/materialize, `MIN` const (skipset's borrow-seed needs it),
  LE-canonical endianness policy (handles are native-endian today).
- blockmap/bimap/skipset/arenaset ports: blocked on MIN + materialize + the
 areniset value story (see doa/CLAUDE.md Future Work).
- block-set capacity ergonomics: explicit byte-cap constructor
  (`with_byte_cap`), DEFAULT_SLOTS retune for 1M+ benches.
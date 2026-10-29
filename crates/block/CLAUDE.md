# block — serializable fixed-storage blocks

The problem: a database index needs blocks that serialize as raw bytes, keep
every operation O(1), never leak stale memory into the serialized form, and
still allow arbitrary user types to move in and out via handles. Every
mechanism in this crate is a consequence of taking those four constraints
seriously.

## The concepts, ranked

1. Relptr + dynamic trailing data (worldview) + FIXED u16 WIDTH. `Block` is
   a 16-byte header plus a dynamically-sized data region in ONE allocation,
   named by byte offsets (`S`/`H`) from the allocation's ends — no const
   generics on data size, and the offset survives any move of the whole box.
   Growth reallocs; the relptr absorbs it. The pointer width is `u16`, HARD-
   PINNED (2026-10-28 user call: padding `P` up to 8 bytes defeated the
   point of narrow handles; the later growable block type will be a near
   verbatim sibling crate with u64s — do NOT macro-generate both). Cap ≤
   65520; the freelist head owns the data region's last 4 bytes, so usable
   front space is `cap − NODE`.
2. Two pointer spaces, TYPED and UNFORGEABLE (worldview + safety). `S<T>` =
   (start byte offset, byte len), stack-relative; `H<T>` = (back-to-start
   offset, byte len), heap-relative, naming a region of `T`. Fields are
   crate-private and constructible only by Block: a value of `H<T>` PROVES
   its region was T-aligned at allocation, so typed reads (`heap::<T>`) carry
   no runtime alignment check and expose no unsafe — stale-handle bounds
   canaries remain. Both survive `grow` — the heap tail slides to the new
   back (H's offsets are back-relative), the stack never moves.
3. The zero-record invariant (the crate's law). A serialized record contains
   ONLY live data, hole bookkeeping, or zero: born `alloc_zeroed`, `grow`
   wipes the realloc'd tail, `push`/`push_buf` zero ALIGNMENT GAPS, and
   every reclaim (`stack_shrink`, `pop_heap`, the archive unwind, hole
   surgery via `sync_holes`) wipes what it rolls back. Deleted user bytes
   never reach a file — this is a security property, not a niceness.
   `alloc` memsets nothing: regions claim zeroed gap.
4. HANDLE LAYOUT LAW (sharp edge, marker-enforced). `Block::push` memcpy's
   `size_of::<T>()` bytes bitwise — Rust moves copy padding too, so implicit
   padding would carry uninit bytes into records. The gate is two marker
   traits: `ZeroPad` (every byte of a constructed value is defined — no
   implicit padding, or construction defines all bytes) and `ReprC`.
   `push<T: Copy + ZeroPad + ReprC>`; `Archive::Handle` carries the bounds.
   Handwritten handles assert it with explicit `Pad<N>` fields; the DERIVE
   proves it — pads computed as const expressions over real
   `size_of`/`align_of` of the projected handles, pinned by a const canary
   (layout mismatch = compile error), and the same proof emits the marker
   impls. This closed the old nightly wall (trait consts as array lengths)
   and the closed layout table: the derive now handles ANY field shape,
   including derive-on-derive.
5. Archive v2 stores, RelOrd compares (separation + field-wise). `Archive<P>`
   has no Ord bound: `archive` allocates each field through the Allocator
   (which pads — archive computes nothing) and returns a fixed-size POD
   Handle (+ `HEAP: bool` const; + `release` for children-first recursive
   frees). Comparisons are `RelOrd`'s `rel_cmp`/`rel_cmp_h`, FIELD-WISE,
   short-circuiting — no view construction; the user rejected views for
   inlining + wasted fields. Capacity feedback = try-then-UNWIND (2026-10-28
   user design; snapshot/rewind is DEAD): every single alloc is atomic by
   construction, so a failed multi-alloc impl releases the children it built
   (children first — `release_slice`, with the partial handle + write count
   as the undo log) then `Allocator::rollback` rolls the front to the entry
   position; archive impls never free on the success path (the unwind is
   the only freeer inside archive). Success pays NO atomicity tax; failure
   pays the frees. No preflight sizing call exists anywhere.
6. Stack = LIFO full-erasure storage. Each stack unit is
   `(heap-front u32, Handle)` serialized IN the record; `pop` shrinks (wipe)
   then `pop_heap`s to the unit's front (wipe) — deleted items leave no
   bytes anywhere in the file, with zero freelist machinery, because the
   front rides the record and `from_record` needs no extra state. Record
   stride = 16 + cap; `RECORD` 4096 ⇒ cap 4080.
7. Stacks = per-block RwLock cells, no owner thread (2026-10-27; the kanal
   actor's ~650ns RTT floor was ~90% of every op). One `Cell` per record —
   `RwLock<Slot{stack, dirty}>` + an atomic lease counter; `Shared` (one
   mutex) owns top/cell table/LRU/file_len. LOCK ORDER shared → cell → file,
   plus one cell → shared edge (residency register) — sound because the only
   shared-holder that touches cells, the evictor, never BLOCKS on one
   (try_write only). `read` closures and lease grants are READ-FIRST:
   residency is checked under the read guard and the closure/grant runs
   there — a resident block can't stop being resident under a read lock
   (the evictor's try_write fails against it), so the fast path has no
   eviction window and same-block readers run parallel with no
   Send/'static bounds; a miss escalates to the write path, where
   `ensure`'s is-none recheck under the lock is the TOCTOU double (a
   racing loader that won first leaves the stack loaded — no double file
   read). Panics unwind naturally (poison recovered via into_inner). Eviction: LRU after each
   op, gated by an atomic resident-count mirror, skips readers + MRU +
   try_write-busy (deferral, never blocks). The last handle's drop IS the
   exit flush (`Inner`'s Drop, synchronous — no thread to race; the
   miri-era async-exit lesson holds by construction). A new block's birth
   record writes immediately (an unflushed block must read back as a valid
   empty record). Push's Cap rule reads BLOCK STATE, not record ownership:
   Err on an EMPTY block under the write lock = oversize; spill re-checks
   `top` under shared so racing producers never double-append (§11).
8. The freelist (v2, in-band + lazy cache). The hole list lives IN the heap
   bytes: a `(len, next)` head node occupies the last NODE bytes of the data
   region permanently (len = 0 marks the head), links are back-offsets sorted
   ascending, `next = 0` ends the list (safe — no hole can reach offset 0
   through the head). Every allocation rounds to ≥ NODE and to a size
   multiple, so every freed region holds its own node and `H::len` may
   exceed the request (tail reads zero; Vec guards by its stored count).
   Block keeps a LAZY `holes: Option<Vec>` cache: loads copy the record and
   nothing else; the first hole-touching op walks the in-band list once
   (bounds-checked, corrupt links truncate there) into the Vec, and every
   later op works the cache, `sync_holes` writing the bytes back so raw
   reads (records, `heap_span`) stay current. `free` binary-inserts and
   coalesces neighbors that TOUCH or sit < NODE apart (a sub-NODE gap is
   provably dead — live regions are ≥ NODE — so the join reclaims dead
   align crumbs and makes unwind near-exact); `alloc` best-fits with
   sub-NODE slacks absorbed as ZEROED dead gaps (dead ≠ dirty, §12);
   `pop_heap` clips straddlers; a hole-path take ZEROES the served span
   (the old node must not reach the caller). Full compaction stays DEFERRED
   (moving live regions stales caller-held H's — needs the handle-fixup
   protocol first).

## Files (lowest → highest)

- `src/lib.rs` — wiring + re-exports.
- `src/handle.rs` — `S<T>`/`H<T>` (unforgeable, u16 fields, private ctor),
  `Pad<N>`, the `ZeroPad`/`ReprC` markers + primitive impls.
- `src/block.rs` — `Block` core: dual-front allocator, Header (repr(C,
  align 16), 16 bytes), the in-band freelist (head node, `NODE`, lazy hole
  cache), wipes, gap zeroing, grow (heap+head slide), `Allocator` (write
  seam: `alloc_slice`, `release_slice`, `write_slot`, `slot`, `heap_len`,
  `rollback`).
- `src/archive.rs` — `Archive`/`RelOrd`, `Heap` read seam (bytes/slots/slot),
  `StrHandle`/`VecHandle` (concrete `Pad` fields), `archive_slice` (the
  recursive slice primitive + its unwind), impls for primitives/char/String
  (8-byte BE prefix cutoff, h.off==0 = inline)/generic Vec<T>.
- `src/stack.rs` — `Stack<T>`: units pushed as (snap u32, handle) two-item
  pairs (per-T const geometry: gap1 after the snap, LEADING gap0 in units
  ≥ 1; `last`/`pop` derive positions BACKWARD from stack_len — the count's
  runtime-stride division stays out of the hot path), record save/load
  (`LoadErr`, stride canary, header sanity; byte-for-byte loads, no field
  decode).
- `src/stacks.rs` — threaded `Stacks` manager (per-block RwLock cells + one
  structural mutex; `Lease`, budgets, eviction, `DEFAULT_BUDGET` 8,
  multi-record files).
- `benches/stacks.rs` — the wall-budget overhead bench; carries the dissolved
  channel transport's echo A/B (`echo-rtt-floor` vs `echo-rtt-floor-kanal`/
  unbounded — the old RTT floor reference for the lock-era cells).

## Testing

Inline `#[cfg(test)]` in the core modules + `tests/stacks.rs` integration
(Stacks' own tests live in the integration suite).
Miri-clean throughout; file-touching tests are gated `#[cfg(not(miri))]` and
the stacks integration suite needs `MIRIFLAGS=-Zmiri-disable-isolation`
(it does real fs I/O). The zero-record invariant, pad law, hole stamping, and
pop erasure each have dedicated pin tests — when touching any of those paths,
the pins must understand why they exist.

## notes

`notes/inception.md` (the substrate's pre-code design writing, moved from
doa's devlog), `notes/subtle_bugs.md` (solved nuances of this crate). Session
record is workspace-level.

## Future Work

- zero-copy record path: the allocation IS the record (header at base, gap
  always zero by the invariant) — `as_record(&self) -> &[u8]` over the whole
  allocation is byte-identical to `Stack::to_record` today; `save()` becomes
  one write of the slice, and a consuming `into_record()` via
  `Vec::from_raw_parts` is possible (Drop disarm). Unbenched, unpinned.
- doa's `BlockSet` port to the v3 API (P-generics + forged `S<P>` fields —
  the doa lib build is broken until this lands; needs its own design pass
  for the unforgeable-handle world).
- Stacks multi-producer ceiling: pushes serialize on the top cell's write
  lock (inherent — one append frontier) plus the entry shared mutex; 8-thread
  aggregate ~210 vs ~90 ns/item single-thread. Top-local batching or a cheaper
  structural path are the candidates; unbenched. `len()` pays a with_slot
  round-trip per block (O(blocks) structural trips) — same family, unbenched.
- Block compaction with a handle-fixup protocol (deltas-style move reporting).
- Archive: view/materialize, `MIN` const (skipset's borrow-seed needs it),
  LE-canonical endianness policy (handles are native-endian today).
- blockmap/bimap/skipset/arenaset ports: blocked on MIN + materialize + the
 areniset value story (see doa/CLAUDE.md Future Work).
- block-set capacity ergonomics: explicit byte-cap constructor
  (`with_byte_cap`), DEFAULT_SLOTS retune for 1M+ benches.
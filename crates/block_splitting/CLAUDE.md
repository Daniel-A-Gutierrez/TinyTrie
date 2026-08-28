# block_splitting

A 4-bit (nibble, 0..16) toy address space prototyping how a fixed-size, pointer-addressed
**block** (the array/leaf node of an index) can **grow and split** while keeping two
invariants true at once: (1) handed-out **virtual addresses** stay stable across
grow/spread, and (2) **physical slot order tracks vaddr order**. The bounded 4-bit space
makes every translator parameter combination exhaustively testable. It is the
stripped-down research ancestor of `doa`'s translator/store/block tiers — no fn-ptr
specialization, no strategies, no tree, just the core `p2v`/`v2p` math and the
grow/spread/split primitives it enables. A binary (`main.rs`), no deps, `#![allow(dead_code)]`.

The central tension worked out here (see `notes.md`): a split can maintain **pointers**
or **ordering**, not both, unless it cuts at the midpoint — and wrapping canonical sets
(`shift > 0` + `inner != 0`) force a buffered, cyclic-arc split rather than an in-place move.

- `nibble.rs` — the 4-bit unsigned int underpinning all address math.
- `translator.rs` — `v2p`/`p2v` address translation over the nibble space + `spread`/`rotate`.
- `block.rs` — `Block<T>`: a `Vec<Option<T>>` + translator + cap; grow/spread/split primitives.
- `main.rs` — demos (`main`) + the one surviving print-probe test (`rotate_range`).

## nibble.rs

A 4-bit unsigned integer (`Nibble`, 0..=15), inner value invariant-masked to the low 4
bits. Foundation for all address math; upholds only the numeric contract (wrapping
arith, 4-bit rotates, truncating shifts). Operator impls (`Add`/`Sub`/`Mul`/`BitAnd`/
`BitOr`/`BitXor`/`Not`/`Shl`/`Shr`) are kept for API symmetry, not used by the
translator/block logic (which calls the `wrapping_*`/`rotate_*` methods directly).

- `Nibble(pub u8)` — the 4-bit int. consts: `BIT_WIDTH=4`, `CAP=16`, `ZERO`/`ONE`/`MAX=15`/`MIDPOINT=8`.
- `from_u8`/`from_usize`/`as_u8`/`as_usize` — masked conversions.
- `wrapping_add`/`wrapping_sub`/`wrapping_mul` — mod-16 arith (mask after).
- `rotate_left`/`rotate_right` — rotate within the 4-bit window.
- `wrapping_shl`/`wrapping_shr` — truncating shift (bits past bit 3 dropped).

## translator.rs

Virtual↔physical address translation, concrete (no fn-ptr specialization — that's `doa`'s
job). `inner_offset` lives in **physical** space (added before the shift); `outer_offset`
in **virtual** space (added after the rotation) — the split lets a block pin its anchor
independently of vaddr placement. `p2v` rotates RIGHT so the vaddr follows the physical
spread; `v2p` is the exact inverse on canonical (block-handed-out) vaddrs.

Invariant: `p2v(p) = ((p + inner) << shift).rotate_right(rot) + outer`; `v2p` inverts it.
With `shift > 0`, `p2v` is injective only on the canonical phys range `[0, cap−inner)`
(the shift drops high bits); past it, `{v, v+8}` collide. `spread` halves `shift` and
re-anchors `inner = 2*inner − offset` so handed-out vaddrs stay stable; `rotate` bumps
`rotation` by 1 (the split partner to `spread`). `len << shift` must not overflow the
bit width or rotation can't recover the bits.

- `Translator { inner_offset, outer_offset, shift, rotation }` — the four parameters; `Clone`/`Copy`/`Debug`.
- `new`/`default` — `default` = identity (all zero, shift 0).
- `p2v`/`v2p` — phys↔vaddr, exact inverse on canonical vaddrs.
- `spread(offset: bool)` — `shift -= 1`, `inner = 2*inner − offset`; panics at `shift == 0`.
- `rotate()` — `rotation = (rotation + 1) % BIT_WIDTH`.

## block.rs

`Block<T>` = a `Vec<Option<T>>` of `len` slots + a `Translator` + a `cap` (max `len`).
Operates on **phys** slots; the caller translates vaddr→phys via `translator().v2p()`.
`len = min(MAX_CAP >> shift, cap)`, so `log2(len) + shift <= bit_width` — len is decoupled
from shift, and a cap-constrained block can be **full at `shift > 0`** (must split, not spread).

Invariants: `occupied ≤ len ≤ cap ≤ MAX_CAP`. The payload is `Node { v, val }` — **two
separate fields**, the key to off-midpoint splits: `v` is the self/vaddr (the pointer field,
the one `block[phys].v == p2v(phys)` / `v2p(v) == phys` is about — the round-trip pointer
invariant); `val` is the ordering payload that travels with the node (parent order is by
`val`). At the midpoint, `v`-order and `val`-order coincide; off-midpoint they diverge — the
`v`s read e.g. `6,14,7,15` (disordered) while the `val`s stay ordered — and that's fine,
because lookups go through `v`/`v2p` and the tree orders on `val`. The "split can keep pointers
*or* ordering, not both" tension only bites when `v` and `val` are conflated; with them
separate, `fixup` (re-stamp `v` to `p2v(phys)` after each move) keeps the pointer invariant
while the choreography reorders `val`. `spread` doubles `len` and moves `phys i → 2i+offset`,
dropping `shift` (vaddrs stable) — reclaims a **low** bit as gap space. A split is the
split-time dual of `spread`: when `shift` hits 0 the low bits are exhausted, so the block
gives its top vaddr half to a sibling and **rotates** to reclaim the now-constant high bit
(rotated into the low position) as the fresh gap bit. The two alternate forever. Rotation
runs only at `shift == 0` (stride 1, where the 15/0 wrap seam has no vaddr between it) and
never spreads — it buffers and remaps via `v2p`; the `i → 2i` spread runs only at `shift > 0`
(stride ≥ 2, where the seam can't be a consecutive placed pair, so the spread always gaps a
real midpoint). Root constructors (`uniform`/`pluripotent`) are built directly, not via
`new`, and deliberately start `len = 1` (under-filled vs `new`'s formula) because they grow
by push/spread.

- `MAX_CAP = 16` — the address-space ceiling.
- `Node { v, val }` — the payload: `v` = self/vaddr (pointer field, `fixup`-stamped); `val` = ordering payload (travels). `Display` shows `val`.
- `rsplit_translator(old, first_phys)` — free fn (rotation split helper): rotate `old` by 1 + re-anchor `inner` so the child's `p2v(0) == old.p2v(first_phys)`. `p2v` evaluated on `old` (pre-rotate); only the `rotate_left` uses the new rotation. Asserts `old.shift == 0` (rotation is the shift-exhausted case). Used by both children in `split_and_rotate`.
- `rsplit_translator_last(old, last_phys)` — free fn: mirror anchoring the *last* element at the top phys (`p2v(MAX_CAP-1) == old.p2v(last_phys)`). Used for the right-bigger half of an off-midpoint split so its `inner=0` and the wrap stays out of the pre-filled low `[0, excess*2)` range.
- `Block<T> { buf, translator, occupancy, cap }` — the block.
  - `new(translator, cap)` — `len = min(MAX_CAP >> shift, cap)`; asserts `shift ≤ BIT_WIDTH`, `cap ∈ [1, MAX_CAP]`.
  - `uniform()` — root: `shift = bw` (len 1), `cap = MAX_CAP`, no offsets; grows by spread, full at `shift == 0`.
  - `pluripotent()` — root: `shift = 2`, len 1, `cap = 4`, `outer = 8`; grows by push front/back/middle (canonical set wraps `{8,12,0,4}`), full at `len == cap` while `shift > 0`. push_front bumps **outer** (not inner) since inner lives pre-shift and would overflow at `shift > 0`.
  - `translator`/`cap`/`len`/`is_vacant`/`occupancy` — accessors.
  - `insert`/`get`/`remove`/`iter` — raw phys-slot ops; `insert`/`remove` panic on occupied/empty/OOB.
  - `spread(offset)` — double `len`, `phys i → 2i+offset`; panics if `2*len > cap` (at cap, must split) or `shift == 0`.
  - `spread_in_place(offset, count)` — the move + `translator.spread` with no `len` growth; shared by `spread` (after pushing `None`s) and `split_shift` (moves into the just-emptied high half).
  - `split_shift(at)` (private) — shift split (`shift > 0`): right child born with `shift - 1` (stride gaps, no rotation), `inner` re-anchored at `p2v(at)`; left spreads in place into the emptied `[at, len)` (no `len` growth, so a cap-constrained block can split without calling `spread`). Reclaims a low bit. (Still generic — the only split op that doesn't need the `Node` payload.)
- `Block<Node>` — payload-specific extras (the split family lives here so it can call `fixup`/`swap_open`):
  - `split(at)` — dispatcher: `shift > 0` → `split_shift` (free low bit), else `split_and_rotate` (free high bit). `at` is the phys split point.
  - `split_and_rotate(at)` — rotation split (`shift == 0`): left keeps `[0, at)`, right `[at, len)`; both rotate + re-anchor (reclaim the high bit). Midpoint = standard drain + reinsert (both halves `val`-ordered). Off-midpoint: drain the smaller half out, preemptively place the seam range (`excess*2` items) at its final end, then compact the rest to its rotated phys — one `swap_open` + `fixup` per move (`v` re-stamped each step, no deferred fixups; parent `val`-order maintained, no `val` comparison). The bigger child is anchored to `inner=0` (left-bigger first-anchored; right-bigger last-anchored via `rsplit_translator_last`). Asserts `shift == 0`; panics if excess `e = |at - mid| >= mid/2`. Precondition: full, `val`-ordered block.

  - `fill()` — seed every empty phys with `Node { v: p2v(phys), val: phys }` (`v` = canonical vaddr, free to wrap; `val` = phys-rank order key, so the block is `val`-ordered for any `inner`); no growth.
  - `fixup(phys)` — re-stamp the `v` field at `phys` to `p2v(phys)` (`val` untouched). The re-key hook `compact_*`/`swap_open`/the split choreography apply after each move; a real node impl recomputes the key vs the new vaddr (here `v` is the key). Panics if `phys >= len` or empty; occupancy unchanged.
  - `compact_left(n)`/`compact_right(n)` — carve `n` free at the front/back by bubbling the nearest `n` frees there via adjacent `None`↔`Some` swaps (minimal, stable — trailing/leading gaps stay, `val` travels with each swapped node), then `fixup` the moved range (`[n, last_r+1)` / `[first_r, len-n)`) so the `v`-invariant holds. Panics if `n > len` or fewer than `n` free.
  - `swap_open(src, open)` — relocate the node at `src` into the open slot `open` (swap — `val` travels), then `fixup` re-stamps `v` to `p2v(open)`. Off-midpoint fixup: one relocated node per call. Panics if `src` empty or `open` occupied/oob; occupancy unchanged.
  - `put(v)` — insert `Node { v, val: v }` at `v2p(v)`, auto-spread-and-retry (drives its own growth); panics at cap.
  - `push_back(v)` — append `Node { v, val: v }` at phys `len` (vaddr `p2v(len)`); no translator change; panics at cap.
  - `push_front(v)` — prepend at phys 0: bump offset down one stride (`inner -= 1` at `shift == 0`, `outer -= 1<<shift` at `shift > 0`), shift items `i -> i+1`; vaddrs preserved; panics at cap.
  - `front_vaddr`/`back_vaddr` — the vaddr the next push_front/back will occupy.

## main.rs

Demos + the one surviving test. `main` fills a block to cap and splits it at the midpoint,
printing both halves with their invariant status. `rotate_range` is a **print probe, not an
asserting test** — it builds a rotated translator and prints `v2p`/`p2v` arrays for eyeballing
(leftover from development; kept by author's choice, hence its unused-variable warnings).
`put`/`check_invariant` are helpers shared with `main`.

- `put`/`check_invariant`/`ordered` — helpers: `put` forwards to `Block::put`; `check_invariant` verifies the `v`-field pointer invariant (`v2p(v)==phys && p2v(phys)==v`); `ordered` checks `val` strictly increasing in phys order (parent order).
- `fw_print_arr` — fixed-width array printer used by `rotate_range`.
- `main` — full-block fill + midpoint `split_and_rotate` demo (prints both halves; `Block` `Display` shows `val`).
- `tests::ordered_after_split` — midpoint rotate + shift split both keep `val`-order + invariant.
- `tests::off_midpoint_left_fixup` — `at ∈ {9,10,11}` (left bigger): `val`-order restored on both halves, pointer invariant held; `at=12` (excess ≥ mid/2) panics.
- `tests::off_midpoint_right_fixup` — `at ∈ {7,6,5}` (right bigger): `val`-order restored on both halves, pointer invariant held; `at=4` (excess >= mid/2) panics.
- `tests::rotate_range` — print-only translator probe (no assertions; unused-variable warnings by author's choice).
- `tests::compact_demo`/`compact_minimal_move` — compact carves a free run, preserves `val`-order, re-keys `v`.
- `off_midpoint_offsets` — off-midpoint split across parent `inner` {0,4,8} × `rotation` 0..4 × all feasible `at` (72 combos); both halves `val`-ordered + pointer-invariant for any parent translator, children carrying varying `inner`.

## Status

Compiles, 7 tests. Realized: the nibble/translator/block tiers; `Node { v, val }` payload
(v = pointer field fixup-stamped to `p2v(phys)`; val = ordering payload that travels) — the
self/val split that lets an off-midpoint split keep *both* the pointer invariant and
`val`-order; `spread` (low-bit gap reclamation); the `split` dispatcher routing `shift > 0` →
`split_shift` (low-bit) vs `shift == 0` → `split_and_rotate` (high-bit), shared
`spread_in_place`/`rsplit_translator`/`rsplit_translator_last` helpers; the off-midpoint
split choreography for both sides (drain the smaller half out, preemptively place the seam
range at its final end, compact the rest to its rotated phys, one `swap_open`+`fixup` per
move with `v` re-stamped each step so no deferred fixups; the bigger child anchored to
`inner=0` — left-bigger first-anchored, right-bigger last-anchored) for `|at - mid| <
mid/2`. Compact (`compact_left`/`compact_right`) carves a free run by bubbling the nearest
`n` frees via `None`↔`Some` swaps (minimal, stable, no clones) then `fixup`s the moved range.

Not wired: graduation into `doa`'s specialized tiers. Wrapping offsets are accepted as
unavoidable (see `notes.md`).

# Updating the Claude.md
Keep the structure a breadth first tree of the subsections - at the top the 'root' tells a reader what this crate is and what its purpose is. There should then be a 1 line description of each subsection that will follow.
Each section afterward should include a brief overview of what the file in the codebase contains, as well as what its broad purpose is, namely what invariants it maintains. Each trait and type defined in a file should be listed, 1 or 2 lines each. 
The document should be ordered so as to maintain a contextual foothold for the reader - the root covered big picture, so ordering the subsections to build up from the lowest level -> highest level makes sense logically. 
Maintain this section at the end of the claude.md .
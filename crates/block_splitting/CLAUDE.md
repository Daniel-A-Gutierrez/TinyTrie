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

Invariants: `occupied ≤ len ≤ cap ≤ MAX_CAP`; for every occupied phys `i`,
`v2p(block[i]) == i` AND `p2v(i) == block[i]` (translator and contents are mutual inverses,
so slot order == vaddr order — the invariant every split defends). `spread` doubles `len`
and moves `phys i → 2i+offset`, dropping `shift` (vaddrs stable). Splits partition the
canonical **cyclic** phys sequence into two arcs and re-anchor each child so its first
vaddr lands at phys 0. Root constructors (`uniform`/`pluripotent`) are built directly,
not via `new`, and deliberately start `len = 1` (under-filled vs `new`'s formula) because
they grow by push/spread.

- `MAX_CAP = 16` — the address-space ceiling.
- `anchored(old, first_phys)` — free fn: rotate `old` by 1 + re-anchor `inner` so the child's `p2v(0) == old.p2v(first_phys)`. `p2v` evaluated on `old` (pre-rotate); only the `rotate_left` uses the new rotation. Shared by both split children.
- `Block<T> { buf, translator, occupancy, cap }` — the block.
  - `new(translator, cap)` — `len = min(MAX_CAP >> shift, cap)`; asserts `shift ≤ BIT_WIDTH`, `cap ∈ [1, MAX_CAP]`.
  - `uniform()` — root: `shift = bw` (len 1), `cap = MAX_CAP`, no offsets; grows by spread, full at `shift == 0`.
  - `pluripotent()` — root: `shift = 2`, len 1, `cap = 4`, `outer = 8`; grows by push front/back/middle (canonical set wraps `{8,12,0,4}`), full at `len == cap` while `shift > 0`. push_front bumps **outer** (not inner) since inner lives pre-shift and would overflow at `shift > 0`.
  - `translator`/`cap`/`len`/`is_vacant`/`occupancy` — accessors.
  - `insert`/`get`/`remove`/`iter` — raw phys-slot ops; `insert`/`remove` panic on occupied/empty/OOB.
  - `spread(offset)` — double `len`, `phys i → 2i+offset`; panics if `2*len > cap` (at cap, must split) or `shift == 0`.
  - `spread_in_place(offset, count)` — the move + `translator.spread` with no `len` growth; shared by `spread` (after pushing `None`s) and `split_and_shift` (moves into the just-emptied high half).
  - `split_from_to(from, to)` — **general split**: left = arc `[from, to)`, right = arc `[to, from)`; `from > to` means a wrapping arc `[from, len) ∪ [0, to)`. Buffers both halves, re-inserts into anchored children (no in-place move, so wrapping arcs/scattered dests are fine). For `shift > 0` the cut must separate collision pairs `{v, v+8}` or the children collide. Sets `self.translator` before the re-insert loop (no transient state).
  - `split_and_rotate(at)` = `split_from_to(0, at)` — the full (`shift == 0`) case.
  - `split_and_shift(at)` — for `shift > 0`: right child born with `shift − 1` (stride gaps, no rotation), `inner` re-anchored at `p2v(at)`; left spreads in place into the emptied `[at, len)` (no `len` growth, so a cap-constrained block can split without calling `spread`).
- `Block<u8>` — payload-specific extras:
  - `fill()` — seed every empty phys with its decoded vaddr `p2v(phys)`, so `block[i] == p2v(i)`; no growth.
  - `put(v)` — insert `v` at `v2p(v)`, auto-spread-and-retry (drives its own growth); panics at cap.
  - `push_back(v)` — append at phys `len` (vaddr `p2v(len)`); no translator change; panics at cap.
  - `push_front(v)` — prepend at phys 0: bump offset down one stride (`inner -= 1` at `shift == 0`, `outer -= 1<<shift` at `shift > 0`), shift items `i → i+1`; vaddrs preserved; panics at cap.
  - `front_vaddr`/`back_vaddr` — the vaddr the next push_front/back will occupy.

## main.rs

Demos + the one surviving test. `main` fills a block to cap and splits it at the midpoint,
printing both halves with their invariant status. `rotate_range` is a **print probe, not an
asserting test** — it builds a rotated translator and prints `v2p`/`p2v` arrays for eyeballing
(leftover from development; kept by author's choice, hence its unused-variable warnings).
`put`/`check_invariant` are helpers shared with `main`.

- `put`/`check_invariant` — thin helpers: `put` forwards to `Block::put`; `check_invariant` verifies the both-direction `v2p`/`p2v` inverse over occupied slots.
- `fw_print_arr` — fixed-width array printer used by `rotate_range`.
- `main` — full-block fill + midpoint `split_and_rotate` demo.
- `tests::rotate_range` — print-only translator probe (no assertions).

## Status

Compiles, 1 test (`rotate_range`, print-only). Realized: the nibble/translator/block tiers;
`spread`, the general cyclic-arc `split_from_to` (handles wrapping canonical sets), the
`shift > 0` `split_and_shift`, shared `spread_in_place`/`anchored` helpers. The
`put`/`push_front`/`push_back` growth paths and the both-direction invariant are exercised
by the (now-deleted) torture harness over `uniform` and `pluripotent` roots.

Not wired: `split_and_hollow` (carve out the near-midpoint slots that need manual fixup when
`at` isn't the midpoint and cap = `PTR::MAX + 1` — the larger half can't spread via rotate
and maintain ordering), off-midpoint split pointer fixup via a walker, graduation into `doa`'s
specialized tiers. Wrapping offsets are accepted as unavoidable (see `notes.md`).

# Updating the Claude.md
Keep the structure a breadth first tree of the subsections - at the top the 'root' tells a reader what this crate is and what its purpose is. There should then be a 1 line description of each subsection that will follow.
Each section afterward should include a brief overview of what the file in the codebase contains, as well as what its broad purpose is, namely what invariants it maintains. Each trait and type defined in a file should be listed, 1 or 2 lines each. 
The document should be ordered so as to maintain a contextual foothold for the reader - the root covered big picture, so ordering the subsections to build up from the lowest level -> highest level makes sense logically. 
Maintain this section at the end of the claude.md .
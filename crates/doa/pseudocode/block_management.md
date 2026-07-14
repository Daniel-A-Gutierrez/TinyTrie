```rust
// Block-level management ops — the primitives the arena's `execute` runs. Each
// performs a phys/address mutation and reports the ptr transform it applied, so
// the arena can hand it back as an ArenaInsertDelta. ArenaInsertDelta is the
// union of these return types, derived bottom-up.
//
// Per op: what's done, the vptr + phys effect, what's returned.

// ---------------------------------------------------------------------
// Transform vocabulary
// ---------------------------------------------------------------------

// Linear — closed-form remap, one variant per shift direction.
//   Left{shift,offset}:   new = (old << shift) + offset
//   Right{shift,offset}:  new = (old >> shift) + offset
// Right is lossless for live addrs: a live addr sits on the source grid (a
// multiple of 2^old_shift), so the >> by (old_shift-new_shift) drops only zero
// bits. Lossy only for stale/garbage addrs the consumer shouldn't hold.
// Right requires old_shift > new_shift (so old_shift >= 1); from an append/
// prepend source (shift 0) every carve is Left or identity — nothing denser to
// go to. Identity = Left{0,0}; spread = Left{1,0}; recenter = Left{0,-delta}.
//
// Right originates in exactly two cases:
//  1. specialize pluripotent -> append/prepend: old_shift = half_ptr-log2(cap)
//     > 0, new_shift = 0. (Pluripotent -> random is Left: random's full-range
//     shift > pluripotent's half-range shift.)
//  2. split_off_pluripotent carved from a random block (Random's
//     AddressExhaustion low-occupancy branch), when random's addr_shift >
//     pluripotent's, i.e. cap_old/cap_new < 2^half_ptr — a small, spread random
//     source. A near-full random block has addr_shift ~= 0 and the carve flips
//     back to Left.
enum Linear {
    Left  { shift: u32, offset: isize },
    Right { shift: u32, offset: isize },
}

// InBlockShift — in-block shift anchored at the insert hint: `count` elements
// `side` the hint each move by `amount`.
struct InBlockShift { count: usize, side: Side, amount: isize }
enum Side { Before, After }

// Chunk — a partition piece: ptrs whose old addr fell in `old_range` now live at
// (new_block, transform.apply(old)).
struct Chunk { old_range: (isize, isize), new_block: usize, transform: Linear }

// ---------------------------------------------------------------------
// Ops
// ---------------------------------------------------------------------

// spread — space elements out for address headroom.
//   phys p -> p<<1 ; v_offset *= 2 ; addr_shift unchanged.
//   vptr: new = old * 2  -> Linear::Left{1,0}. Same block.
//   Only when addresses aren't exhausted (×2 would overflow PTR) — OutOfBudget,
//   not AddressExhaustion. Returns Readdressed.
fn spread(block: &mut Block) -> Readdress

// recenter — slide the address window; phys untouched.
//   v_offset += delta.
//   vptr: new = old - delta  -> Linear::Left{0,-delta}. Same block.
//   Returns Readdressed.
fn recenter(block: &mut Block, delta: isize) -> Readdress

// grow — realloc VecDeque to new_cap. No vptr change (capacity only).
//   Substep of grow_and_spread; rarely a standalone delta.
fn grow(block: &mut Block, new_cap: usize)

// grow_and_spread — grow 2x then spread.
//   vptr: new = old * 2  -> Linear::Left{1,0}. Same block.
//   Random OutOfBudget high-occ cap<max. Returns Readdressed.
fn grow_and_spread(block: &mut Block) -> Readdress

// split_in_two — partition phys at `at` into two linked blocks.
//   A: phys [0,at) unchanged. B: phys [at,len) reindexed to [0,len-at),
//   B.virt_offset = parent.virt_offset - (at<<addr_shift)  -> phys_to_virt
//   identical across the split.
//   vptr: unchanged in both halves  -> Linear::Left{0,0}. Halving density opens
//   gaps (fixes OutOfBudget); addresses kept, so doesn't fix end-exhaustion.
//   Returns Split { new_loc: <half with anchor>, chunks:[A Left{0,0}, B Left{0,0}] }.
fn split_in_two(block: &mut Block, at: usize) -> Split

// split_off_pluripotent — carve phys [carve_start, carve_start+cap) into a fresh
//   pluripotent block (cap, new_shift = log2(half)-log2(cap)); reindex to [0,cap).
//   Remainder stays (gap left None or closed by an in-block shift).
//   vptr (carved chunk):
//     new = ((old + v_off_old - (carve_start<<old_shift)) <op> (new_shift-old_shift)) - v_off_new
//     <op> = << if new_shift>=old_shift (Left) else >> (Right, lossless).
//   Worked example: dense parent (shift 0, v_off_old 70) carve phys 64..96 ->
//     i16 pluripotent cap 32 (new_shift 3, v_off_new 128):
//     new = (old<<3) - 80  -> Linear::Left{3,-80}.
//   Returns Split { new_loc: pluripotent,
//                   chunks:[carve Linear, remainder Linear::Left{0,0}] }.
//   Remainder may add a Shifted if its gap got closed.
fn split_off_pluripotent(block: &mut Block, range: Range<usize>, cap: usize) -> Split

// split_mid — [left, pluripotent mid, right]. Left/right keep the grid; mid is
//   pluripotent (Linear per split_off_pluripotent). Mirror of split_off but the
//   carve is the middle and both tails stay linked.
//   Returns Split { new_loc: mid,
//                   chunks:[left Left{0,0}, mid Linear, right Left{0,0}] }.
fn split_mid(block: &mut Block, at: usize, cap: usize) -> Split

// shove — move block's `end` element to the neighbor's opposite end, freeing a
//   slot at `end` for the insert.
//   phys: one element out to neighbor; freed slot takes the new value.
//   vptr: shoved element -> (neighbor, new addr there); insert stays in the
//   original block at the freed slot. Fallback new_block_between if the neighbor
//   can't represent the shoved addr (address exhaustion — len==cap just
//   reallocs, not a failure).
//   Returns Shoved { new_loc: original, shoved_to_id, shoved_to_idx, shift }.
fn shove(block: &mut Block, end: End, neighbor: &mut Block) -> Shoved

// new_block_between — shove fallback: fresh block between block and its neighbor
//   in direction `end`; shove into it.
//   vptr: shoved element -> (new block, addr in new grid).
//   Returns Shoved { new_loc: original, shoved_to_id: <new block>, ... }.
fn new_block_between(block: &mut Block, end: End) -> Shoved

// specialize — pluripotent -> concrete at len == half_ptr (its AddressExhaustion
//   remediation; the insert failed, so new_virt is recomputed fresh in the new
//   layout). Re-lay elements densely under the concrete addr_shift.
//   vptr: uniform readdress  -> Linear (Left if new_shift>=old_shift, else Right).
//   Returns Readdressed. Same block.
fn specialize(block: &mut Block) -> Readdress

// ---------------------------------------------------------------------
// ArenaInsertDelta — union of the op return types. new_loc is DECOUPLED from the
// moves: the new element may land in the original block (shove) or a new one
// (split), independent of which chunk moved where.
// ---------------------------------------------------------------------

enum ArenaInsertDelta {
    Free { block_id: usize, new_virt: isize },                          // gap/push, no move
    Shifted { block_id: usize, new_virt: isize, shift: InBlockShift },  // in-block shift
    Readdressed { block_id: usize, new_virt: isize, transform: Linear },// whole-block readdress
    Split { new_loc: (usize, isize), chunks: Vec<Chunk> },              // partitioned
    Shoved {
        new_loc:       (usize, isize),            // insert, in original block
        shoved_to_id:  usize,
        shoved_to_idx: isize,
        shift:         Option<InBlockShift>,     // in-block shift that closed the freed slot
    },
}

// ---------------------------------------------------------------------
// Notes
//
// Right is lossless for live addrs (a live addr is a multiple of 2^shift in the
// shifted frame), so carves/specializes into a smaller addr_shift emit Right and
// need no pre-densification — O(1) per held pointer, exact. It originates in
// exactly two cases (see Transform vocabulary above): specialize pluripotent ->
// append/prepend, and split_off_pluripotent carved from a sufficiently spread
// (small) random block.
//
// new_loc decoupling: Split/Shoved carry the new element's location separately
// from the moves (shove -> original; split-in-2 -> 50/50 which half).
//
// One remediation per insert: each op runs at most once via `execute`. A
// remediation composing two ops (e.g. split_off_pluripotent + gap-close) folds
// the Shifted into the Split as a chunk with old_block==new_block and a
// Linear::Left{0, ±amount}.
//
// Open: address wrapping within a block would let split-in-2 (and append/prepend)
// reuse the half-range grid-preserving splits currently leave as headroom.
```
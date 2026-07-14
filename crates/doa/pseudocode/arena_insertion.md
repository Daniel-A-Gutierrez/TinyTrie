```rust
// Arena-level insert. hint = (block_id, in-block addr). Tries the block layer;
// on the NotFound the block surfaces by design, asks its strategy for a
// remediation plan, executes it (arena owns storage — the strategy only gets
// &mut Block), and places the value per the plan. Effectively infallible.
//
// Strategy owns the decision (`plan_remediation`); arena owns execution.
impl<T, U, I> Arena<T, U, I>
where U: UnsignedIndex, I: SignedBlockIndex, T: Sized {

    fn insert_before(&mut self, hint: (U, I), value: T) -> ArenaInsertDelta {
        let (block_id, addr) = hint;
        let block = &mut self.blocks[block_id.as_usize()];
        let anchor = block.virt_to_phys(addr.as_isize());

        // Strategy fast-path. Append/prepend skip find_slot when the hint is at
        // their end: append -> push_back (anchor==len && back representable),
        // prepend -> push_front (anchor==0 && front representable). fast_path
        // returns the Found find_slot would produce, without scanning.
        // Pluripotent/random opt out (None) -> normal find_slot.
        let fast = block.strategy.fast_path(&*block, anchor);

        let result = match fast {
            Some(found) => block.insert_found(anchor, found, value, Bias::Left),
            None        => block.try_insert_before(anchor, value),
        };
        match result {
            Ok(block_delta) => {
                self.record_hint(hint);
                lift(block_id.as_usize(), block_delta, hint)   // Free/Shifted, same block
            }
            Err(not_found) => {
                // execute runs the remediation AND places `value` (placement shape
                // depends on the plan: split retries into a new block;
                // grow_and_spread/specialize retry in the same block at a
                // re-translated anchor; shove places at the freed slot in the
                // original), returning the full ArenaInsertDelta. One remediation
                // per insert; a second NotFound inside execute is an invariant break.
                let plan = block.strategy.plan_remediation(&*block, not_found, anchor);
                self.record_hint(hint);
                self.execute(plan, block_id.as_usize(), anchor, value)
            }
        }
    }

    fn insert_after(&mut self, hint: (U, I), value: T) -> ArenaInsertDelta {
        // mirror of insert_before: Bias::Right; fast-path append at anchor==len-1,
        // prepend at anchor==0 if frontmost occupied.
        todo!("mirror of insert_before")
    }

    // -- made-up primitives (bodies not defined here) --
    // record_hint(hint): push_back onto self.requests, pop_front if len > 16.
    // execute(plan, block_id, anchor, value) -> ArenaInsertDelta:
    //   run the remediation + place `value`, returning the matching variant
    //   (Split/Readdressed/Shoved/Shifted) with new_virt filled in. Arena-only.
    //
    // Block entry points (block_management.md):
    //   insert_found(anchor, found, value, bias): handle_insertion + post_insert
    //     with a precomputed Found (skips find_slot). Fast-path feeds it.
    //   try_insert_before/after(anchor, value): find_slot + insert_found.
    //
    // Strategy::fast_path(&block, anchor) -> Option<Found>:
    //   Append:     anchor == buf.len() && back_viable  -> Some(Append)
    //   Prepend:    anchor == 0 && front_viable         -> Some(Prepend)
    //   Pluripotent/Random: None
}

// ArenaInsertDelta, Chunk, Linear, InBlockShift, Side are defined in
// block_management.md (authoritative, bottom-up). Not redefined here.
//
// BlockInsertDelta — the BLOCK layer's contract (no split variant; the block
// refuses via NotFound, splitting is the arena's job).

enum BlockInsertDelta {
    Free { new_virt: isize },                                        // gap/push, address-stable
    Move { new_virt: isize, amount: usize, direction: isize, addr_delta: isize }, // in-block shift; addr_delta = direction << addr_shift
}

// Lift a block-layer delta to the arena layer. The block's absolute Move
// re-expresses as the hint-anchored InBlockShift (the arena knows the hint).
fn lift(block_id: usize, block_delta: BlockInsertDelta, hint: (U, I)) -> ArenaInsertDelta {
    match block_delta {
        BlockInsertDelta::Free { new_virt } =>
            ArenaInsertDelta::Free { block_id, new_virt },
        BlockInsertDelta::Move { new_virt, amount, direction, addr_delta } =>
            ArenaInsertDelta::Shifted {
                block_id, new_virt,
                shift: InBlockShift { count: amount,
                                      side: side_from_hint(hint, direction),
                                      amount: addr_delta },
            },
    }
}

pub struct Arena<T, U, I: SignedBlockIndex> {
    blocks:   VecDeque<Block<T, I>>,
    requests: VecDeque<(U, I)>,
}

// Remediation — the plan a strategy returns from `plan_remediation` (what to
// do, given NotFound + anchor + occupancy + recent-insert pattern); the arena
// executes it. Which block/phys to place into is resolved by `execute`.

enum Remediation {
    GrowAndSpread,                        // in-place: bigger buf + spread. Same block, retry at re-translated anchor. (Random OutOfBudget high-occ cap<max.)
    SplitMid { at: usize },               // [left, pluripotent mid, right]; retry mid. (Append/Prepend OutOfBudget dense middle.)
    SplitInTwo,                           // partition; retry the chunk with the anchor. (Random cap==max.)
    SplitOffPluripotent { take: Take },   // carve a pluripotent out; retry it. (Append/Prepend AddressExhaustion; Random low-occ no-pattern.)
    NewBlockBetween,                       // shove fallback: fresh block between block and neighbor; shove into it. Gate: neighbor address-exhausted (len==cap just reallocs, not a failure).
    /// Pluripotent filled up (len == half_ptr): specialize to a concrete strategy.
    /// Its AddressExhaustion remediation (budget = whole block, can't OutOfBudget).
    /// NotFound-triggered, so the insert failed (no stale new_virt); retry is fresh.
    Specialize,
    Shove { end: End },                   // block's `end` element -> neighbor's opposite end; insert takes the freed slot in the original block.
}

enum End { Front, Back }

enum Take {
    Front(usize),   // frontmost N
    At(usize),      // around anchor
    End(usize),     // endmost N
    Empty,          // empty pluripotent at the end (Append exhaustion End case)
}

// plan_remediation — per-strategy decision trees (strategy has &Block + the hint
// queue, no block ids). Predicates: occupancy, cap, max_of, locate(anchor)->
// Front|Mid|End, recent_inserts_sequential (scan self.requests), last_insert_site.
//
// Pluripotent:  Specialize                                  // can't OutOfBudget; only AddressExhaustion
//
// Append:  OutOfBudget => SplitMid { at: anchor }           // Front/End would've pushed, not refused
//          AddressExhaustion => match locate(anchor) {
//            Front => SplitOffPluripotent { take: Front(64) }
//            Mid   => SplitOffPluripotent { take: At(anchor) }
//            End   => SplitOffPluripotent { take: Empty } }
//
// Prepend: mirror of Append (Front<->End, Front(64)<->End(64), Empty at Front).
//
// Random:  OutOfBudget =>
//            if occupancy > 0.75 { if cap < max { GrowAndSpread } else { SplitInTwo } }
//            else if recent_inserts_sequential { SplitOffPluripotent { take: At(last_insert_site) } }
//            else if cap < max { GrowAndSpread } else { SplitInTwo }
//          AddressExhaustion =>
//            if occupancy > 0.75 { SplitInTwo } else { SplitOffPluripotent { take: At(anchor) } }

// =====================================================================
// RECAP
//
// Contract: insert runs internal ops that re-represent the consumer's
// (block_id, addr) ptrs, returns a remap the consumer applies.
//
// - One remediation per insert (no loop).
// - Remap: Linear{Left|Right} (Left = <<, Right = lossless >> for live addrs).
//   Carves/specializes into a smaller addr_shift use Right, no pre-densification.
// - Specialize = pluripotent's AddressExhaustion remediation (NotFound-triggered
//   at len==half_ptr; retry fresh).
// - split_in_two: B reindexes + shifts virt_offset by -(at<<addr_shift) ->
//   both chunks Linear::Left{0,0}, no phys waste.
// - split_off_pluripotent carve transform: new = ((old + v_off_old -
//   (carve_start<<old_shift)) <op> (new_shift-old_shift)) - v_off_new, <op>=<<(Left)
//   or >>(Right). Ex: dense parent v_off_old 70, carve 64..96 -> pluripotent cap32
//   -> Linear::Left{3,-80}.
// - Shove: insert stays in original block at freed slot; shoved element to
//   (neighbor_or_new_block, addr).
//
// Open: address wrapping would reclaim the half-range grid-preserving splits
// leave as headroom. 
// T: ArenaNode trait so the arena updates pointers internally.
//
// Next: implement the block_management.md ops in src/block.rs.
```
```
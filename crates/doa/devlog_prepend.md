## 2026-09-15 — terminology unification: addr / pos / ChildPos / Rel

One vocabulary now, crate-wide (code + CLAUDE.md + subtle_bugs.md). Entries below
this one still use the old words — map them:

| old (below) | now |
|---|---|
| vaddr, virt, virtual, vptr, `P` (the generic) | address; trait `Addr` (ex-`BlockIndex`); generic param `A` |
| phys, physical, `p` | position; newtype `Pos(usize)` |
| child idx / gap index (bare usize) | newtype `ChildPos(usize)` |
| `v2p` / `p2v` / `vdist` | `a2p` / `p2a` / `adist` |
| `vget` / `first_vaddr` / `last_vaddr` / `root_vaddr` | `aget` / `first_addr` / `last_addr` / `root_addr` |
| `fix_p` / `affects_p` / `fix_v` / `affects_v` | `fix_pos` / `affects_pos` / `fix_addr` / `affects_addr` |
| `as_halfptr` / `from_halfptr` | `as_half` / `from_half` |
| `before: bool` / `after: bool` / `dir: bool` (side args) | `Rel { Before, After }` |
| `SignedBlockIndex`, `SignedNum` | removed — unused |

Semantics, so old entries still parse: `Pos` is the slot in the store's array
(the truth — pos 0 = min, pos len−1 = max); `Addr` (u16/u32) is the stable name
that survives relocation; `ChildPos` is a slot in the parent's child sequence, and
as an insertion gap it may equal `child_count`. Positions carry derived `Ord` +
usize-rhs `+`/`-`, so hot-path arithmetic reads bare; everything else is `.0`.
`Rel` encodes the side of an anchor, replacing every side bool — the `!before`
inversions died with it. `Fixable` keeps one method per fixup kind
(`grew_fix`/`swap_fix`/`slide_fix`/`two_slide`, by-value fixup args — `GrewFixup`
went `Copy` for that); the generic `fixup<F: Fixup + ?Sized>` call sites were
migrated onto them. `ascend` returns `(parent node, ChildPos)`. `CursorState`
(position / reposition / descend, `Clone` for the run-walk snapshot) got defined
— that finishes the migration the tree was dirty with mid-September. The
store's reservation model (`MaybeUninit` write-places) had already been retired
by then: slots are plain `Option<T>`, values always initialized.

Uncompiled pending a port to this surface: `leafblock.rs`, `inline_leafblock.rs`
(mod decls commented out in lib.rs) and `src/tests/{store,walker}.rs` (the
`#[cfg(test)]` wirings in store.rs/walker.rs are 2-line comments; re-enable after
porting).
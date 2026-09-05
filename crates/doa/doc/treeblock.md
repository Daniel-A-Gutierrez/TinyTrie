```rust
//!`TreeBlock` — a block whose stored type is a node: the param-less marker
//!trait + the free-fn constructors `walker`/`search` (over the consumer's
//!`From` impls — local type, orphan-safe). `SplitTreeBlock` is the declared
//!sketch of the arena-level cleave, unwired.
///L0011
macro_rules! impl_tree_block;
///L0029
///tree block: a block whose stored type is a node. param-less marker — construction
///lives on the free fns `walker`/`search` via the consumer's `From` impls
///(`impl From<&'a MyBlock> for MyCursor` — local type, orphan-safe), so no walker
///family params dangle at call sites. crate-impl'd for `Block` per mode (both
///`TreeBlock` and `Block` are doa's).
pub trait TreeBlock<'block>: BlockTrait<'block> + BlockOps<'block>
where
    Self::N: Node,
    Self::BlockData: HasRoot<Self::P>,
{
    ///phys slot of the root node. default: `BlockData::root`.
    fn root_position(&self) -> usize;
}
///L0043
///block-level splits (cleave on `BlockExhausted`, arena handoff) — declared,
///unwired. the node-level split driver lives on `SplitTreeWalker` (it needs the
///walker's fixup machinery); this surface is the future arena tier's.
pub trait SplitTreeBlock<'block>: BlockTrait<'block> + BlockOps<'block>
where Self::N: SplittableNode
{
    ///cleave the block; returns the separator the caller wires under an arena
    ///parent when the block itself splits.
    fn split_root(&mut self) -> <Self::N as Node>::K;
}
///L0054
///walker at the block's root (shared). `R` is the borrow — `&B` or `&mut B` — so one
///fn covers shared and mut walkers: the `From` impl the consumer names picks it.
///`NW` must be ascend-capable (`TreeWalk` traverses).
pub fn walker<'block, NW, B, R>(b: R) -> TreeWalker<B::O, NW>
where
    B: BlockTrait<'block> + 'block,
    B::N: Node,
    NW: NodeWalker<'block, B> + From<R>,
    R: std::ops::Deref<Target = B>,
;
///L0066
///walker routed to `k`'s terminal node. stackless cursors work here (`search` needs
/// descent only); `walker` for a positioned-at-root start.
pub fn search<'block, NW, B, R>(b: R, k: &<B::N as Node>::K) -> TreeWalker<B::O, NW>
where
    B: BlockTrait<'block> + 'block,
    B::N: Node,
    NW: NodeCursor<'block, B> + From<R>,
    R: std::ops::Deref<Target = B>,
;
///L0078
impl_tree_block!(crate::blocks::Uniform);
///L0079
impl_tree_block!(crate::blocks::Pluripotent);
///L0080
impl_tree_block!(crate::blocks::Anchored<O>);
```

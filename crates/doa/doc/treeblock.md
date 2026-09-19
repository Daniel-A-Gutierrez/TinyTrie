```rust
//!`TreeBlock` — a block whose stored type is a node: the param-less marker
//!trait (root access + the arena-tier `split_block` sketch, `todo!`) + the
//!free-fn constructors `walker`/`search` (over the consumer's `From` impls —
//!local type, orphan-safe).
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
    Self::BlockData: HasRoot<Self::A>,
{
    ///position of the root node. default: `BlockData::root`.
    fn root_position(&self) -> Pos;
    ///hand the root to the node at `pos` (root promotion). block-data level
    /// only — tree re-wiring is the consumer's.
    fn set_root(&mut self, pos: Pos);
    ///cleave a full block in two. the current root pops out — extracted, its
    /// slot reclaimed — and `left_root`/`right_root` install at the two blocks'
    /// root positions. returns (left, right, popped root, split boundary): the
    /// popped root's child entries partition as `[0..m]` left, `[m..]` right
    /// (key order = child order ⇒ prefix). the consumer drains the popped root
    /// into the two new roots via `get_mut(root_position())`.
    fn split_block(
        self,
        _left_root: Self::N,
        _right_root: Self::N,
    ) -> (Self, Self, Self::N, usize);
}
///L0061
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
///L0073
///walker routed to `k`'s terminal node. stackless cursors work here (`search` needs
/// descent only); `walker` for a positioned-at-root start.
pub fn search<'block, NW, B, R>(b: R, k: &<B::N as Node>::K) -> TreeWalker<B::O, NW>
where
    B: BlockTrait<'block> + 'block,
    B::N: Node,
    NW: NodeCursor<'block, B> + From<R>,
    R: std::ops::Deref<Target = B>,
;
///L0085
impl_tree_block!(crate::blocks::Uniform);
///L0086
impl_tree_block!(crate::blocks::Pluripotent);
///L0087
impl_tree_block!(crate::blocks::Anchored<O>);
```

use crate::blocks::{BlockOps, BlockTrait};
use crate::walker::{
    Node, NodeCursor, NodeWalker, NodeWalkerMut, SplittableNode, TreeWalker,
};

///tree block: a block whose stored type is a node, naming the consumer's walker types.
///the consumer implements this for their block alias (`MyWalker walks Block<MyNode>`) —
/// supplying the two GATs + `root_position` is the whole impl; the constructors below
/// come for free. `root_position`: `Anchored` derives it from the translator; movable-
/// root modes read it from `BlockData` (that impl then bounds `Self::BlockData: HasRoot`).
pub trait TreeBlock<'block>: BlockTrait<'block> + BlockOps<'block>
where
    Self::N: Node + Default,
{
    ///consumer shared walker (GAT over the block borrow).
    type NW<'walker>: NodeWalker<'block, 'walker, Self>
    where
        'block: 'walker,
        Self: 'walker;
    ///consumer mut walker.
    type NWM<'walker>: NodeWalkerMut<'block, 'walker, Self>
    where
        'block: 'walker,
        Self: 'walker;

    ///phys slot of the root node.
    fn root_position(&self) -> usize;

    ///bare consumer cursor at the root (no wrapper) — stackless, lookup-only use.
    fn cursor<'a>(&'a self) -> Self::NW<'a>
    where
        'block: 'a,
    {
        <Self::NW<'a> as NodeCursor<'block, 'a, Self>>::from_block(self)
    }

    ///bare consumer mut cursor at the root.
    fn cursor_mut<'a>(&'a mut self) -> Self::NWM<'a>
    where
        'block: 'a,
    {
        <Self::NWM<'a> as NodeWalkerMut<'block, 'a, Self>>::from_block_mut(self)
    }

    ///walker at the root (shared).
    fn walker<'a>(&'a self) -> TreeWalker<Self::O, Self::NW<'a>>
    where
        'block: 'a,
    {
        TreeWalker::new(self.cursor())
    }

    ///walker routed to `k`'s terminal node (shared).
    fn lookup<'a>(&'a self, k: &<Self::N as Node>::K) -> TreeWalker<Self::O, Self::NW<'a>>
    where
        'block: 'a,
    {
        let mut w = self.walker();
        let _ = w.nw.walk_to(k);
        w
    }

    ///walker at the root (mut).
    fn walker_mut<'a>(&'a mut self) -> TreeWalker<Self::O, Self::NWM<'a>>
    where
        'block: 'a,
    {
        TreeWalker::new(self.cursor_mut())
    }

    ///walker routed to `k`'s terminal node (mut).
    fn lookup_mut<'a>(&'a mut self, k: &<Self::N as Node>::K) -> TreeWalker<Self::O, Self::NWM<'a>>
    where
        'block: 'a,
    {
        let mut w = self.walker_mut();
        let _ = w.nw.walk_to(k);
        w
    }
}

///splits (clone-split driver, root promotion, block cleave) — declared, unwired.
///see the split design notes; requires `SplittableNode` nodes.
pub trait SplitTreeBlock<'block>: TreeBlock<'block>
where
    Self::N: SplittableNode + Default,
{
    ///split the root node; promotes a new root. returns the separator the caller wires
    ///under an arena parent when the block itself also splits.
    fn split_root(&mut self) -> <Self::N as Node>::K;
}
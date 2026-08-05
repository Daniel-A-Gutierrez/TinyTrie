use std::marker::PhantomData;

use crate::alloc_strat::AllocStrat;
use crate::block::{BlockCursor, BlockCursorMut, BlockMutTrait, BlockTrait, OpenSlot, RawBlock};
use crate::node::Node;
use crate::store::Store;
use crate::translator::Translator;
use crate::{Ordering, index::*};
pub struct TreeBlock<'a, T, P, A, S, O, Meta>
where
    T: Sized + 'a,
    P: BlockIndex,
    A: AllocStrat<P>,
    S: Store<'a, T>,
    O: Ordering,
    Meta: Sized + 'static,
{
    meta:  Meta,
    block: RawBlock<'a, T, P, A, S>,
    _o:    PhantomData<O>,
}

pub(crate) trait TreeBlockMut<'a>: BlockMutTrait<'a> + 'a
where Self::T: Node
{
    type Meta;
    type K;
    type V;
    type O;
    fn meta(&self) -> &Self::Meta;
    fn set_meta(&mut self, m: Self::Meta);
}

impl<'a, T, P, A, S, O, Meta> BlockTrait<'a> for TreeBlock<'a, T, P, A, S, O, Meta>
where
    T: Sized + 'a,
    P: BlockIndex,
    A: AllocStrat<P>,
    S: Store<'a, T> + 'a,
    O: Ordering,
    Meta: Sized + 'static,
{
    type T = T;
    type P = P;
    type S = S;
    type Cursor<'b>
        = BlockCursor<'a, 'b, Self>
    where 'a: 'b;

    fn store<'b>(&'b self) -> &'b Self::S
    where 'a: 'b {
        self.block.store()
    }

    fn translator<'b>(&'b self) -> &'b Translator<Self::P> {
        self.block.translator()
    }

    fn cursor<'b>(&'b self) -> Self::Cursor<'b>
    where 'a: 'b {
        BlockCursor::new(self)
    }

    ///delegate so REVERSED strategies still iterate high→low.
    fn iter<'b>(&'b self) -> impl ExactSizeIterator<Item = &'b Self::T> + 'b
    where 'a: 'b {
        self.block.iter()
    }
}

impl<'a, T, P, A, S, O, Meta> BlockMutTrait<'a> for TreeBlock<'a, T, P, A, S, O, Meta>
where
    T: Sized + 'a,
    P: BlockIndex,
    A: AllocStrat<P>,
    S: Store<'a, T> + 'a,
    O: Ordering,
    Meta: Sized + 'static + Default,
    RawBlock<'a, T, P, A, S>: BlockMutTrait<'a, A = A> + BlockTrait<'a, T = T, P = P, S = S>,
{
    type A = A;
    type CursorMut<'b>
        = BlockCursorMut<'a, 'b, Self>
    where 'a: 'b;
    fn new() -> Self {
        Self { meta: Meta::default(), block: RawBlock::new(), _o: PhantomData }
    }

    fn store_mut(&mut self) -> &mut Self::S {
        self.block.store_mut()
    }
    fn translator_mut(&mut self) -> &mut Translator<Self::P> {
        self.block.translator_mut()
    }

    fn cursor_mut<'b>(&'b mut self) -> Self::CursorMut<'b>
    where 'a: 'b {
        BlockCursorMut::new(self)
    }

    fn insert(&mut self, v: Self::T, slot: OpenSlot) -> Self::P {
        self.block.insert(v, slot)
    }

    fn split(&mut self) -> Self {
        Self { meta: Meta::default(), block: self.block.split(), _o: PhantomData }
    }
    fn split_and_rotate(&mut self) -> Self {
        Self {
            meta:  Meta::default(),
            block: self.block.split_and_rotate(),
            _o:    PhantomData,
        }
    }

    fn try_insert_back(&mut self, v: Self::T) -> Result<Self::P, Self::T> {
        self.block.try_insert_back(v)
    }
    fn try_insert_front(&mut self, v: Self::T) -> Result<Self::P, Self::T> {
        self.block.try_insert_front(v)
    }
}

impl<'a, T, P, A, S, O, Meta> TreeBlockMut<'a> for TreeBlock<'a, T, P, A, S, O, Meta>
where
    T: Sized + Node + 'a,
    P: BlockIndex,
    A: AllocStrat<P>,
    S: Store<'a, T> + 'a,
    O: Ordering,
    Meta: Sized + 'static + Default,
    RawBlock<'a, T, P, A, S>: BlockMutTrait<'a, A = A> + BlockTrait<'a, T = T, P = P, S = S>,
{
    type Meta = Meta;
    type K = T::K;
    type V = T::V;
    type O = O;
    fn meta(&self) -> &Meta {
        return &self.meta;
    }
    fn set_meta(&mut self, m: Meta) {
        self.meta = m
    }
}

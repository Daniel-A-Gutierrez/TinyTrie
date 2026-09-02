use crate::{Ordering, RootPos, index::*, metadata::{Fixable, FoundSlot}, store::{DequeStore, NoneSlide, Store, VecStore}, translator::{AddressTranslator, Translator}, walker::{Node, TreeWalker}};

use std::marker::PhantomData;

///no-pin full-range block (anchor 0, no insertion pin). for trees that grow by splitting
///(the root can't stay at a fixed position anyway) and other consumers that don't pin.
pub struct Uniform;
///root pinned at a fixed vaddr determined by `O` (preorder=0, inorder=MIDPOINT,
///postorder=MAX); `find_slot`/`slide_none` implicitly pin `v2p(root_vaddr)`. the caller
///has no choice but to pin.
pub struct Anchored<O: Ordering>(PhantomData<O>);
///sprase block with an overprovisioned pointer type capable of push front/back + insert. 
pub struct Pluripotent;
///dense `push_back` only (with a periodic None gap for mid-inserts). pin=None.
pub struct Append;
///`Append` mirrored: dense `push_front` only (= physical `push_back`), iteration high→low. pin=None.
pub struct Prepend;
pub struct Block<'block, T, P, S, M, D, O>
{
    store: S,
    translator: Translator<P>,
    block_data: D,
    _phantom: PhantomData<(&'block T, M, O)>,
}

pub struct InsufficientMaxCapacity();

pub struct OpenSlot(pub usize);

pub type UniformBlock    <'block,T,P : BlockIndex,D,O:Ordering> = Block<'block,T,P,VecStore<T>,  Uniform,D,O>;
pub type PluripotentBlock<'block,T,P : BlockIndex,D,O:Ordering> = Block<'block,T,P,DequeStore<T>,Pluripotent,D,O>;
pub type AnchoredBlock   <'block,T,P : BlockIndex,D,O:Ordering> = Block<'block,T,P,VecStore<T>,  Anchored<O>,D,O>;
pub type AppendBlock     <'block,T,P : BlockIndex,D,O:Ordering> = Block<'block,T,P,VecStore<T>,  Append,D,O>;
pub type PrependBlock    <'block,T,P : BlockIndex,D,O:Ordering> = Block<'block,T,P,VecStore<T>,  Prepend,D,O>;
//doesnt compile because o is unused on the right
//type UTreeBlock<'block,T,P:BlockIndex,D:Root,O:Ordering> = Block<'block,T,P,VecStore<T>,Uniform,D>;

pub trait Mode<P:BlockIndex> : 'static {
    const INNER_OFFSET : P = P::ZERO;
    const OUTER_OFFSET : P = P::ZERO;
    const SHIFT : u32 = 0;
    const INIT_CAP : usize = 1;
    const MAX_CAP : usize = 1<<P::BIT_WIDTH;
    const REVERSED : bool = false;
    fn make_translator() -> Translator<P> {
        Translator::new(Self::INNER_OFFSET,Self::OUTER_OFFSET,Self::SHIFT,0)
    }
}

pub trait BlockTrait<'block>  : Sized {
    type T: Sized + 'block;
    type P: BlockIndex;
    type S: Store<'block, Self::T> + 'block;
    ///per-block payload (e.g. `BTreeMeta<P>{height, root}` for tree blocks; `()` otherwise).
    type BlockData: Fixable<Self::P>;
    type O : Ordering;

    fn store<'b>(&'b self) -> &'b Self::S
    where 'block: 'b;
    fn translator<'b>(&'b self) -> &'b Translator<Self::P>;
    fn data(&self) -> &Self::BlockData;

    ///physical get. panics if the slot is `None` (caller guarantees `p` occupied).
    fn get<'b>(&'b self, p: usize) -> &'b Self::T
    where 'block: 'b {
        self.store().get(p)
    }
    ///virtual get: translate vaddr→phys. panics if the slot is `None`.
    fn vget<'b>(&'b self, ptr: Self::P) -> &'b Self::T
    where 'block: 'b {
        self.store().get(self.translator().v2p(ptr))
    }
    ///vaddr of first occupied slot, None if empty.
    fn first_vaddr<'b>(&'b self) -> Option<Self::P>
    where 'block: 'b {
        let s = self.store();
        let tr = self.translator();
        for p in 0..s.len() {
            if s.slot(p).is_some() {
                return Some(tr.p2v(p));
            }
        }
        None
    }
    ///vaddr of last occupied slot, None if empty.
    fn last_vaddr<'b>(&'b self) -> Option<Self::P>
    where 'block: 'b {
        let s = self.store();
        let tr = self.translator();
        for p in (0..s.len()).rev() {
            if s.slot(p).is_some() {
                return Some(tr.p2v(p));
            }
        }
        None
    }
    fn v2p(&self, virt: Self::P) -> usize {
        self.translator().v2p(virt)
    }
    fn p2v(&self, phys: usize) -> Self::P {
        self.translator().p2v(phys)
    }
    fn vdist(&self, v1: Self::P, v2: Self::P) -> usize {
        self.translator().vdist(v1, v2)
    }
    fn occupied<'b>(&'b self) -> usize
    where 'block: 'b {
        self.store().occupied()
    }
    fn len<'b>(&'b self) -> usize
    where 'block: 'b {
        self.store().len()
    }
    fn cap<'b>(&'b self) -> usize
    where 'block: 'b {
        self.store().cap()
    }

    // ---- mut surface ----
    fn store_mut(&mut self) -> &mut Self::S;
    fn translator_mut(&mut self) -> &mut Translator<Self::P>;
    fn set_data(&mut self, m: Self::BlockData);

    ///physical mut get. panics if the slot is `None`.
    fn get_mut<'b>(&'b mut self, p: usize) -> &'b mut Self::T
    where 'block: 'b {
        self.store_mut().get_mut(p)
    }
    ///virtual mut get. panics if the slot is `None`.
    fn vget_mut<'b>(&'b mut self, ptr: Self::P) -> &'b mut Self::T
    where 'block: 'b {
        let p = self.translator().v2p(ptr);
        self.store_mut().get_mut(p)
    }
    ///two disjoint `&mut` to occupied physical slots. panics if `a == b` or either is `None`.
    fn get_disjoint_mut<'b>(&'b mut self, a: usize, b: usize) -> (&'b mut Self::T, &'b mut Self::T)
    where 'block: 'b {
        self.store_mut().get_disjoint_mut(a, b)
    }
    fn remove(&mut self, p: usize) -> (Self::T, OpenSlot) {
        (self.store_mut().remove(p), OpenSlot(p))
    }
    fn swap(&mut self, a: usize, b: usize) {
        self.store_mut().swap(a, b);
    }
    ///swap the record at phys `src` with the None at `open`. returns the slot freed at
    ///`src`'s phys and the phys the record moved to.
    fn swap_open(&mut self, src: usize, open: OpenSlot) -> (OpenSlot, usize) {
        self.store_mut().swap(src, open.0);
        (OpenSlot(src), open.0)
    }
}



///(shift, inner_offset, outer_offset) pinning the root at `O`'s fixed vaddr.
const fn fr_params<P: BlockIndex, O: Ordering>() -> (u32, P, P, usize) {
    match O::ROOT_POS {
        RootPos::Beginning => (P::BIT_WIDTH as u32, P::ZERO, P::ZERO,1),
        RootPos::Middle => (P::BIT_WIDTH as u32 - 1, P::ZERO, P::ZERO,2),
        RootPos::End => (P::BIT_WIDTH as u32, P::MIDPOINT, P::MAX,1),
    }
}

impl<P: BlockIndex> Mode<P> for Uniform {const SHIFT : u32 = P::BIT_WIDTH as u32;}
impl<O: Ordering, P: BlockIndex> Mode<P> for Anchored<O> {
    const SHIFT: u32 = fr_params::<P, O>().0;
    const INNER_OFFSET: P = fr_params::<P, O>().1;
    const OUTER_OFFSET: P = fr_params::<P, O>().2;
    const INIT_CAP : usize = fr_params::<P,O>().3;
}
impl<P: BlockIndex> Mode<P> for Pluripotent {
    const SHIFT : u32 = P::BIT_WIDTH as u32 /2;
    const MAX_CAP : usize = 1<<P::Half::BIT_WIDTH;
}
impl<P: BlockIndex> Mode<P> for Append {}
impl<P: BlockIndex> Mode<P> for Prepend {const REVERSED : bool = true;}

///strat-agnostic Self-construction + split primitives + generic `new`/`iter`.
impl<'block, T, P, S, M, D, O> Block<'block, T, P, S, M, D, O>
where
    T: Sized + 'block,
    P: BlockIndex,
    M: Mode<P>,
    S: Store<'block, T> + 'block,
    D: 'block + Default + Clone + Fixable<P>,
    O : Ordering
{
    ///fresh block: empty store + the mode's initial translator + default meta. compile-time
    ///asserts the store's CAP fits the mode's addressable range.
    pub fn new() -> Self
    {
        Self {
            store: S::with_capacity(M::INIT_CAP),
            translator: M::make_translator(),
            block_data: D::default(),
            _phantom: PhantomData,
        }
    }

    ///construct from a store + translator + meta.
    pub fn from_parts(store: S, translator: Translator<P>, block_data : D) -> Self {
        Self { store, translator, block_data, _phantom: PhantomData }
    }

    ///construct from a store + translator + meta.
    pub fn into_parts(store: S, translator: Translator<P>, block_data : D) -> (S,Translator<P>,D) {
        (store, translator, block_data )
    }
}

impl<'block, T, P, S, M, D, O> BlockTrait<'block> for Block<'block, T, P, S, M, D, O>
where
    T: Sized + 'block,
    P: BlockIndex,
    M: Mode<P>,
    S: Store<'block, T> + 'block,
    D: 'block + Default + Clone + Fixable<P>,
    O: Ordering
{
    type T = T;
    type P = P;
    type S = S;
    type BlockData = D;
    type O = O;

    fn store<'b>(&'b self) -> &'b S
    where 'block: 'b {
        &self.store
    }
    fn translator<'b>(&'b self) -> &'b Translator<P> {
        &self.translator
    }
    fn data(&self) -> &D {
        &self.block_data
    }

    fn store_mut(&mut self) -> &mut Self::S {
        &mut self.store
    }
    fn translator_mut(&mut self) -> &mut Translator<Self::P> {
        &mut self.translator
    }
    fn set_data(&mut self, m: Self::BlockData) {
        self.block_data = m;
    }
}

impl<'block, T, P, D, O> PluripotentBlock<'block, T, P, D, O>
where T: Sized + 'block, P: BlockIndex, D: 'block + Default + Clone, O: Ordering {
    fn iter<'b, I>()->I where 'block:'b, I : DoubleEndedIterator<Item=&'b T> + ExactSizeIterator<Item=&'b T> {
        todo!();
    }
    fn find_slot(&mut self, pos : usize, before : bool) -> FoundSlot { todo!() }
    fn slide_none(&mut self, slide: NoneSlide) -> OpenSlot { todo!() }
    fn insert(&mut self, item : T, slot : OpenSlot) -> P { todo!() }
    fn append(&mut self, item : T) -> Result<usize,T> { todo!() }
    fn prepend(&mut self, item : T) -> Result<usize, T> { todo!() }
    //split self into 2 blocks, no fixup, right gets a clone of the left's other parameters.
    fn cleave(self, at : usize) -> (Self,Self) { todo!() }
}

impl<'block, T, P, D, O> AnchoredBlock<'block, T, P, D, O>
where T: Sized + 'block, P: BlockIndex, D: 'block + Default + Clone, O : Ordering {
    fn iter<'b, I>()->I where 'block:'b, I : DoubleEndedIterator<Item=&'b T> + ExactSizeIterator<Item=&'b T> {
        todo!();
    }
    fn find_slot(&mut self, pos : usize, before : bool) -> FoundSlot { todo!() }
    fn slide_none(&mut self, slide: NoneSlide) -> OpenSlot { todo!() }
    fn insert(&mut self, item : T, slot : OpenSlot) -> P { todo!() }
    fn append(&mut self, item : T) -> Result<usize,T> { todo!() }
    fn prepend(&mut self, item : T) -> Result<usize, T> { todo!() }
    //split self into 2 blocks, no fixup, right gets a clone of the left's other parameters.
    fn cleave(self) -> (Self,Self) { todo!() }
}

impl<'block, T, P, D, O> UniformBlock<'block, T, P, D, O>
where T: Sized + 'block, P: BlockIndex, D: 'block + Default + Clone, O: Ordering {
    fn iter<'b, I>()->I where 'block:'b, I : DoubleEndedIterator<Item=&'b T> + ExactSizeIterator<Item=&'b T> {
        todo!();
    }
    fn find_slot(&mut self, pos : usize, before : bool) -> FoundSlot { todo!() }
    fn slide_none(&mut self, slide: NoneSlide) -> OpenSlot { todo!() }
    fn insert(&mut self, item : T, slot : OpenSlot) -> P { todo!() }
    //split self into 2 blocks, no fixup, right gets a clone of the left's other parameters.
    fn cleave(self, at : usize) -> (Self,Self) { todo!() }
}

impl<'block, T, P, D, O> AppendBlock<'block, T, P, D, O>
where T: Sized + 'block, P: BlockIndex, D: 'block + Default + Clone, O: Ordering {
    fn iter<'b, I>()->I where 'block:'b, I : DoubleEndedIterator<Item=&'b T> + ExactSizeIterator<Item=&'b T> {
        todo!();
    }
    fn append(&mut self, item : T) -> Result<usize,T> { todo!() }
    //split self into 2 blocks, no fixup, right gets a clone of the left's other parameters.
    fn cleave(self, at : usize) -> (Self,Self) { todo!() }
}

impl<'block, T, P, D, O> PrependBlock<'block, T, P, D, O>
where T: Sized + 'block, P: BlockIndex, D: 'block + Default + Clone, O: Ordering {
    fn iter<'b, I>()->I where 'block:'b, I : DoubleEndedIterator<Item=&'b T> + ExactSizeIterator<Item=&'b T> {
        todo!();
    }
    fn prepend(&mut self, item : T) -> Result<usize, T> { todo!() }
    //split self into 2 blocks, no fixup, right gets a clone of the left's other parameters.
    fn cleave(self, at : usize) -> (Self,Self) { todo!() }
}

/*

*/
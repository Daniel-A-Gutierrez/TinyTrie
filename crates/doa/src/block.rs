use crate::{Fixup, Ordering, RootPos,
            index::*,
            store::{DequeStore, NoneSlide, Store, VecStore},
            translator::{AddressTranslator, Translator}};
use std::fmt;
use std::fmt::Write as _;
use std::marker::PhantomData;
use crate::block_cursor::*;

///block strategy marker. a type-level tag carried as `PhantomData` on `Block`; each mode
///is a distinct *type* generic over its pointer width `P` (`Uniform<P>` ≠ `FixedRoot<O,P>`),
/// so wrong-usecase calls are compile errors. per-mode: the translator config, the
///addressable CAP limit (a const baked from `P`), and iteration direction.
pub trait Mode: 'static {
    ///the pointer width this mode is specialized for; bound to `Block`'s `P` at the alias.
    type P: BlockIndex;
    ///max store CAP this mode's translator can address, baked from `Self::P`.
    const CAP_LIMIT: usize;
    ///iteration direction: `Prepend` iterates high→low (reversed); others forward.
    const REVERSED: bool = false;
    ///the mode's initial translator config.
    fn new_translator() -> Translator<Self::P>;
}

///no-pin full-range block (anchor 0, no insertion pin). for trees that grow by splitting
///(the root can't stay at a fixed position anyway) and other consumers that don't pin.
pub struct Uniform<P: BlockIndex>(PhantomData<P>);
///root pinned at a fixed vaddr determined by `O` (preorder=0, inorder=MIDPOINT,
///postorder=MAX); `find_slot`/`slide_none` implicitly pin `v2p(root_vaddr)`. the caller
///has no choice but to pin.
pub struct FixedRoot<O: Ordering, P: BlockIndex>(PhantomData<O>, PhantomData<P>);
///half-range, both-ends-growable, root slides. `DequeStore`. pin=None. not generic over
///ordering: push_back/push_front are incompatible with a fixed root position anyway.
pub struct Pluripotent<P: BlockIndex>(PhantomData<P>);
///dense `push_back` only (with a periodic None gap for mid-inserts). pin=None.
pub struct Append<P: BlockIndex>(PhantomData<P>);
///`Append` mirrored: dense `push_front` only (= physical `push_back`), iteration high→low. pin=None.
pub struct Prepend<P: BlockIndex>(PhantomData<P>);

impl<P: BlockIndex> Mode for Uniform<P> {
    type P = P;
    const CAP_LIMIT: usize = 1usize << P::BIT_WIDTH;
    fn new_translator() -> Translator<P> {
        Translator::new(P::ZERO, P::ZERO, P::BIT_WIDTH as u32, 0)
    }
}
impl<O: Ordering, P: BlockIndex> Mode for FixedRoot<O, P> {
    type P = P;
    const CAP_LIMIT: usize = 1usize << P::BIT_WIDTH;
    fn new_translator() -> Translator<P> {
        let (shift, inner) = match O::ROOT_POS {
            RootPos::Middle => (P::BIT_WIDTH as u32 - 1, 0usize),
            RootPos::End => (P::BIT_WIDTH as u32, 1usize << (P::BIT_WIDTH - 1)),
            RootPos::Beginning => (P::BIT_WIDTH as u32, 0usize),
        };
        Translator::new(P::from_usize(inner), P::ZERO, shift, 0)
    }
}
impl<P: BlockIndex> Mode for Pluripotent<P> {
    type P = P;
    const CAP_LIMIT: usize = 1usize << P::Half::BIT_WIDTH;
    fn new_translator() -> Translator<P> {
        Translator::new(
            P::ZERO,
            P::from_usize(1usize << (P::BIT_WIDTH - 1)),
            P::Half::BIT_WIDTH as u32 - 1,
            0,
        )
    }
}
impl<P: BlockIndex> Mode for Append<P> {
    type P = P;
    const CAP_LIMIT: usize = 1usize << P::BIT_WIDTH;
    fn new_translator() -> Translator<P> {
        Translator::new(P::from_usize(1usize << P::Half::BIT_WIDTH), P::ZERO, 0, 0)
    }
}
impl<P: BlockIndex> Mode for Prepend<P> {
    type P = P;
    const CAP_LIMIT: usize = 1usize << P::BIT_WIDTH;
    const REVERSED: bool = true;
    fn new_translator() -> Translator<P> {
        Translator::new(P::from_usize(1usize << P::Half::BIT_WIDTH), P::ZERO, 0, 0)
    }
}

///fixed root vaddr for an ordering (the FixedRoot pin target).
fn root_vaddr<O: Ordering, P: BlockIndex>() -> P {
    match O::ROOT_POS {
        RootPos::Beginning => P::ZERO,
        RootPos::Middle => P::MIDPOINT,
        RootPos::End => P::MAX,
    }
}

///debug rendering aid for a block-stored item. debug-only.
pub(crate) trait SlotDebug<P: BlockIndex> {
    fn debug_render(&self, tr: &Translator<P>) -> Vec<String>;
}

///find_slot result: an optional grow fixup (apply to live phys) + an optional pending
///slide (apply via `slide_none`).
pub struct FoundSlot {
    pub grew: Option<GrewFixup>,
    pub slide: Option<NoneSlide>,
}

pub struct GrewFixup {
    shl: u32,
    shift_offset: u8,
}

pub struct InsufficientMaxCapacity();

impl Fixup for GrewFixup {
    fn fix_p(&self, p: &mut usize) {
        *p <<= self.shl;
        *p += self.shift_offset as usize;
    }
}

///read-only block surface. strat-agnostic: `T`/`P`/`S` derived from the impl. no `iter`
///(iteration direction is per-usecase — `Uniform`/`FixedRoot` forward, `Prepend` reversed).
pub trait BlockBase<'a>: 'a {
    type T: Sized + 'a;
    type P: BlockIndex;
    type S: Store<'a, Self::T> + 'a;
    ///per-block payload (e.g. `BTreeMeta<P>{height, root}` for tree blocks; `()` otherwise).
    type Meta;

    fn store<'b>(&'b self) -> &'b Self::S
    where 'a: 'b;
    fn translator<'b>(&'b self) -> &'b Translator<Self::P>;
    fn meta(&self) -> &Self::Meta;

    ///physical get. panics if the slot is `None` (caller guarantees `p` occupied).
    fn get<'b>(&'b self, p: usize) -> &'b Self::T
    where 'a: 'b {
        self.store().get(p)
    }
    ///virtual get: translate vaddr→phys. panics if the slot is `None`.
    fn vget<'b>(&'b self, ptr: Self::P) -> &'b Self::T
    where 'a: 'b {
        self.store().get(self.translator().v2p(ptr))
    }
    ///vaddr of first occupied slot, None if empty.
    fn first_vaddr<'b>(&'b self) -> Option<Self::P>
    where 'a: 'b {
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
    where 'a: 'b {
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
    where 'a: 'b {
        self.store().occupied()
    }
    fn len<'b>(&'b self) -> usize
    where 'a: 'b {
        self.store().len()
    }
    fn cap<'b>(&'b self) -> usize
    where 'a: 'b {
        self.store().cap()
    }
    fn max_capacity(&self) -> usize {
        Self::S::max_capacity()
    }
}

///strat-agnostic mutation core — only what EVERY block supports regardless of alloc
///strategy: slot accessors, `insert_root` (first insert), and in-place slot edits
///(`get_mut`/`remove`/`swap`/`swap_open`). the mid-insert + split surface
///(`find_slot`/`slide_none`/`grow_and_spread`/`insert`/`split_*`) lives on `SparseBlock`
///(Uniform/Pluripotent/FixedRoot); the push surface (`try_push_*`) lives on the
///per-usecase push traits (Pluripotent/Append/Prepend).
pub trait BlockBaseMut<'a>: BlockBase<'a> {
    fn store_mut(&mut self) -> &mut Self::S;
    fn translator_mut(&mut self) -> &mut Translator<Self::P>;
    fn set_meta(&mut self, m: Self::Meta);

    ///first insert into an empty block. grows to the strat's initial cap and lands the
    ///root at the midpoint phys. returns the root's phys.
    fn insert_root(&mut self, v: Self::T) -> usize;

    ///physical mut get. panics if the slot is `None`.
    fn get_mut<'b>(&'b mut self, p: usize) -> &'b mut Self::T
    where 'a: 'b {
        self.store_mut().get_mut(p)
    }
    ///virtual mut get. panics if the slot is `None`.
    fn vget_mut<'b>(&'b mut self, ptr: Self::P) -> &'b mut Self::T
    where 'a: 'b {
        let p = self.translator().v2p(ptr);
        self.store_mut().get_mut(p)
    }
    ///two disjoint `&mut` to occupied physical slots. panics if `a == b` or either is `None`.
    fn get_disjoint_mut<'b>(&'b mut self, a: usize, b: usize) -> (&'b mut Self::T, &'b mut Self::T)
    where 'a: 'b {
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

///sparse mid-insert + split surface: find/open a slot (`find_slot`/`slide_none`), grow
///(`grow_and_spread`), place (`insert` — None→Some at a slot, distinct from push), and
///split (`split_block`/`split_block_and_rotate`). impl'd by the sparse-capable strats
///(Uniform/Pluripotent/FixedRoot); NOT by the dense push-only strats (Append/Prepend).
///`slide_none`/`insert` are strat-agnostic defaults; `find_slot`/`grow_and_spread`/
///`split_*` are per-Mode. pin is implicit per-Mode (root for FixedRoot, None otherwise).
pub trait SparseBlock<'a>: BlockBaseMut<'a> {
    ///find free slot or make space. `dir` is logical (true=after); `Prepend` flips it to
    ///phys. `pos` is physical. pin is implicit per-Mode.
    fn find_slot(&mut self, pos: usize, dir: bool) -> FoundSlot;
    ///slide the None `ms.from` -> `ms.to`; returns the opened slot. pin=None default;
    ///FixedRoot overrides to pin the root.
    fn slide_none(&mut self, ms: NoneSlide) -> OpenSlot {
        OpenSlot(self.store_mut().slide_none(ms, None))
    }
    ///manually grow + spread; fails if shift==0 or would exceed max capacity.
    fn grow_and_spread(&mut self) -> Result<GrewFixup, InsufficientMaxCapacity>;
    ///place `v` at the opened slot (None→Some). returns its phys. distinct from push.
    fn insert(&mut self, v: Self::T, slot: OpenSlot) -> usize {
        self.store_mut().insert(v, slot.0);
        slot.0
    }
    ///self keeps [0,at). precondition: len == P::MAX.as_usize() + 1 (block full).
    fn split_block(&mut self, at: usize) -> Self;
    ///split at `at` then spread both sides, add 1 rotation to the translator.
    fn split_block_and_rotate(&mut self, at: usize) -> Self;
}

///raw ordered arena run: owns a store + translator + a `Mode` tag, upholds no structural
///invariant. the only concrete block type; per-usecase surfaces are traits impl'd for a
///specific `Mode`.
pub struct Block<'a, T, P, S, M, Meta = ()>
where
    T: Sized + 'a,
    P: BlockIndex,
    M: Mode,
    S: Store<'a, T>,
    Meta: 'a + Default + Clone,
{
    store: S,
    translator: Translator<P>,
    meta: Meta,
    _mode: PhantomData<M>,
    _phantom: PhantomData<&'a T>,
}

pub struct OpenSlot(pub usize);

///emits the per-`Mode` `BlockBaseMut` accessors (identical across all modes; only the
///type params differ). invoked inside each `BlockBaseMut` impl.
macro_rules! block_base_accessors {
    ($S:ty, $P:ty) => {
        fn store_mut(&mut self) -> &mut $S {
            &mut self.store
        }
        fn translator_mut(&mut self) -> &mut Translator<$P> {
            &mut self.translator
        }
        fn set_meta(&mut self, m: Self::Meta) {
            self.meta = m;
        }
    };
}

///emits the strat-agnostic `split_block`/`split_block_and_rotate` (delegate to the
///inherent split primitives). invoked inside each `SparseBlock` impl.
macro_rules! sparse_split {
    () => {
        fn split_block(&mut self, at: usize) -> Self {
            self.split(at)
        }
        fn split_block_and_rotate(&mut self, at: usize) -> Self {
            let v_start = self.p2v(at);
            let v_end = self.p2v(0);
            self.split_and_rotate(v_start, v_end)
        }
    };
}

impl<'a, T, P, S, M, Meta> BlockBase<'a> for Block<'a, T, P, S, M, Meta>
where
    T: Sized + 'a,
    P: BlockIndex,
    M: Mode,
    S: Store<'a, T> + 'a,
    Meta: 'a + Default + Clone,
{
    type T = T;
    type P = P;
    type S = S;
    type Meta = Meta;

    fn store<'b>(&'b self) -> &'b S
    where 'a: 'b {
        &self.store
    }
    fn translator<'b>(&'b self) -> &'b Translator<P> {
        &self.translator
    }
    fn meta(&self) -> &Meta {
        &self.meta
    }
}

///concrete cursor factory on the `Block` struct: the cursor lives here (one impl), not on
///the core traits (a generic `BlockBaseMut` cursor doesn't know the usecase). each
///per-usecase trait extends this to expose its cursor.
pub trait BlockCursorOf<'a>: BlockBase<'a> {
    type Cursor<'cursor>: Cursor<'cursor, Self::T, Self::P>
    where
        'a: 'cursor,
        Self: 'cursor;
    type CursorMut<'cursor>: CursorMut<'cursor, Self::T, Self::P>
    where
        'a: 'cursor,
        Self: 'cursor;
    fn cursor<'cursor>(&'cursor self) -> Self::Cursor<'cursor>
    where 'a: 'cursor;
    fn cursor_mut<'cursor>(&'cursor mut self) -> Self::CursorMut<'cursor>
    where 'a: 'cursor;
}

impl<'a, T, P, S, M, Meta> BlockCursorOf<'a> for Block<'a, T, P, S, M, Meta>
where
    T: Sized + 'a,
    P: BlockIndex,
    M: Mode,
    S: Store<'a, T> + 'a,
    Meta: 'a + Default + Clone,
    Block<'a, T, P, S, M, Meta>: BlockBaseMut<'a>,
{
    type Cursor<'cursor>
        = BlockCursor<'a, 'cursor, Self, &'cursor Self>
    where 'a: 'cursor;
    type CursorMut<'cursor>
        = BlockCursor<'a, 'cursor, Self, &'cursor mut Self>
    where 'a: 'cursor;
    fn cursor<'cursor>(&'cursor self) -> Self::Cursor<'cursor>
    where 'a: 'cursor {
        BlockCursor::new(self)
    }
    fn cursor_mut<'cursor>(&'cursor mut self) -> Self::CursorMut<'cursor>
    where 'a: 'cursor {
        BlockCursor::new(self)
    }
}

impl<'a, T, P, S, M> fmt::Debug for Block<'a, T, P, S, M>
where
    T: Sized + 'a + SlotDebug<P>,
    P: BlockIndex,
    M: Mode,
    S: Store<'a, T> + 'a,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let tr = &self.translator;
        writeln!(
            f,
            "Block {{\n  tr(inner={:?}, outer={:?}, shift={}, rot={})",
            tr.inner_offset(),
            tr.outer_offset(),
            tr.shift(),
            tr.rotation()
        )?;
        let s = self.store();
        let len = s.len();
        let mut buf = String::new();
        buf.push_str("  slots: [");
        for phys in 0..len {
            if phys > 0 {
                buf.push_str(", ");
            }
            match s.slot(phys) {
                Some(item) => {
                    let parts = item.debug_render(tr);
                    let _ = write!(buf, "{phys}:[{}]", parts.join(","));
                }
                None => {
                    let _ = write!(buf, "{phys}:X");
                }
            }
        }
        buf.push(']');
        f.write_str(&buf)?;
        f.write_str("\n}")
    }
}

///forward-or-reverse iterator wrapper: unifies `iter()` and `iter().rev()` behind one
///`impl ExactSizeIterator` so `Block::iter` can branch on `Mode::REVERSED`.
enum EitherIter<L: Iterator, R: Iterator<Item = L::Item>> {
    Fwd(L),
    Rev(R),
}

impl<L: Iterator, R: Iterator<Item = L::Item>> Iterator for EitherIter<L, R> {
    type Item = L::Item;
    fn next(&mut self) -> Option<L::Item> {
        match self {
            EitherIter::Fwd(l) => l.next(),
            EitherIter::Rev(r) => r.next(),
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            EitherIter::Fwd(l) => l.size_hint(),
            EitherIter::Rev(r) => r.size_hint(),
        }
    }
}

impl<L: ExactSizeIterator, R: ExactSizeIterator<Item = L::Item>> ExactSizeIterator
    for EitherIter<L, R>
{
    fn len(&self) -> usize {
        match self {
            EitherIter::Fwd(l) => l.len(),
            EitherIter::Rev(r) => r.len(),
        }
    }
}

///strat-agnostic Self-construction + split primitives + generic `new`/`iter`.
impl<'a, T, P, S, M, Meta> Block<'a, T, P, S, M, Meta>
where
    T: Sized + 'a,
    P: BlockIndex,
    M: Mode,
    S: Store<'a, T> + 'a,
    Meta: 'a + Default + Clone,
{
    ///fresh block: empty store + the mode's initial translator + default meta. compile-time
    ///asserts the store's CAP fits the mode's addressable range.
    pub fn new() -> Self {
        const { assert!(S::CAP <= cap_limit::<P>(M::CAP_KIND), "CAP exceeds Mode CAP_LIMIT"); }
        Self {
            store: S::with_capacity(n),
            translator: M::new_translator::<P>(),
            meta: Meta::default(),
            _mode: PhantomData,
            _phantom: PhantomData,
        }
    }

    ///forward iteration; reversed for `Prepend` (high→low).
    pub fn iter<'b>(&'b self) -> impl ExactSizeIterator<Item = &'b T> + 'b
    where
        'a: 'b,
    {
        let it = self.store.iter();
        if M::REVERSED {
            EitherIter::Rev(it.rev())
        } else {
            EitherIter::Fwd(it)
        }
    }

    ///construct from a store + translator + meta.
    pub(crate) fn from_parts(store: S, translator: Translator<P>, meta: Meta) -> Self {
        Self { store, translator, meta, _mode: PhantomData, _phantom: PhantomData }
    }

    ///split [at,len) into a new block, cloning the translator. caller guarantees no nodes
    ///present in right point to nodes in left.
    pub(crate) fn split(&mut self, at: usize) -> Self {
        debug_assert!(at <= self.store.len(), "split: at out of range");
        let right = self.store.split(at);
        let mut translator = self.translator.clone();
        let at = P::from_usize(at);
        //preserve right-half vaddrs: p2v_new(p-at) == p2v_old(p) => io_new = io_old + at.
        translator.set_inner_offset(self.translator.inner_offset().wrapping_add(at));
        Self {
            store: right,
            translator,
            meta: self.meta.clone(),
            _mode: PhantomData,
            _phantom: PhantomData,
        }
    }

    ///split [v_start, v_end) vaddrs into a new block (rotation-remap). the new block's
    ///translator bumps rotation by 1 (to intersperse free space).
    pub(crate) fn split_and_rotate(&mut self, v_start: P, v_end: P) -> Self {
        let len = self.store.len();
        let cap = S::max_capacity();
        let mut new_trans = self.translator.clone();
        new_trans.set_rotation((self.translator.rotation() + 1) % P::BIT_WIDTH as u32);
        let mut new_store = S::with_capacity(cap);
        let mut i = self.translator.v2p(v_start);
        let end = self.translator.v2p(v_end);
        while i != end {
            let v = self.translator.p2v(i);
            let new_phys = new_trans.v2p(v);
            let elem = self.store.remove(i);
            new_store.insert(elem, new_phys);
            i = (i + 1) % len;
        }
        Self::from_parts(new_store, new_trans, self.meta.clone())
    }
}

// ---------------------------------------------------------------------------
// BlockBaseMut impls — one per Mode. accessors + insert_root only (the mid-insert/split
// surface is on SparseBlock; the push surface is on the per-usecase push traits).
// ---------------------------------------------------------------------------

impl<'a, T, P, const CAP: usize, Meta: 'a + Default + Clone> BlockBaseMut<'a> for Block<'a, T, P, VecStore<T, CAP>, Uniform, Meta>
where
    T: Sized + 'a,
    P: BlockIndex,
{
    block_base_accessors!(VecStore<T, CAP>, P);

    fn insert_root(&mut self, v: T) -> usize {
        assert!(self.store().len() == 0, "insert_root: block not empty");
        self.store_mut().grow_back(1);
        self.store_mut().insert(v, 0);
        0
    }
}

impl<'a, T, P, O: Ordering, const CAP: usize, Meta: 'a + Default + Clone> BlockBaseMut<'a>
    for Block<'a, T, P, VecStore<T, CAP>, FixedRoot<O>, Meta>
where
    T: Sized + 'a,
    P: BlockIndex,
{
    block_base_accessors!(VecStore<T, CAP>, P);

    fn insert_root(&mut self, v: T) -> usize {
        assert!(self.store().len() == 0, "insert_root: block not empty");
        let cap = match O::ROOT_POS {
            RootPos::Middle => 2,
            _ => 1,
        };
        self.store_mut().grow_back(cap);
        let mid = cap / 2;
        self.store_mut().insert(v, mid);
        mid
    }
}

impl<'a, T, P, const CAP: usize, Meta: 'a + Default + Clone> BlockBaseMut<'a>
    for Block<'a, T, P, DequeStore<T, CAP>, Pluripotent, Meta>
where
    T: Sized + 'a,
    P: BlockIndex,
{
    block_base_accessors!(DequeStore<T, CAP>, P);

    fn insert_root(&mut self, v: T) -> usize {
        assert!(self.store().len() == 0, "insert_root: block not empty");
        self.store_mut().grow_back(1);
        self.store_mut().insert(v, 0);
        0
    }
}

impl<'a, T, P, const CAP: usize, Meta: 'a + Default + Clone> BlockBaseMut<'a> for Block<'a, T, P, VecStore<T, CAP>, Append, Meta>
where
    T: Sized + 'a,
    P: BlockIndex,
{
    block_base_accessors!(VecStore<T, CAP>, P);

    fn insert_root(&mut self, v: T) -> usize {
        assert!(self.store().len() == 0, "insert_root: block not empty");
        self.store_mut().grow_back(1);
        self.store_mut().insert(v, 0);
        0
    }
}

impl<'a, T, P, const CAP: usize, Meta: 'a + Default + Clone> BlockBaseMut<'a> for Block<'a, T, P, VecStore<T, CAP>, Prepend, Meta>
where
    T: Sized + 'a,
    P: BlockIndex,
{
    block_base_accessors!(VecStore<T, CAP>, P);

    fn insert_root(&mut self, v: T) -> usize {
        assert!(self.store().len() == 0, "insert_root: block not empty");
        self.store_mut().grow_back(1);
        self.store_mut().insert(v, 0);
        0
    }
}

// ---------------------------------------------------------------------------
// SparseBlock impls — Uniform/Pluripotent/FixedRoot. find_slot/grow_and_spread per-Mode;
// slide_none/insert default (FixedRoot overrides slide_none to pin the root).
// ---------------------------------------------------------------------------

impl<'a, T, P, const CAP: usize, Meta: 'a + Default + Clone> SparseBlock<'a> for Block<'a, T, P, VecStore<T, CAP>, Uniform, Meta>
where
    T: Sized + 'a,
    P: BlockIndex,
{
    sparse_split!();

    fn find_slot(&mut self, mut pos: usize, dir: bool) -> FoundSlot {
        let mut found = FoundSlot { grew: None, slide: None };
        let shift = self.translator().shift();
        if self.occupied() * 3 > self.len() * 4 && shift > 0 {
            if let Ok(g) = self.grow_and_spread() {
                g.fix_p(&mut pos);
                found.grew = Some(g);
            }
        }
        if let Some(ns) = self.store().find_slot(pos, dir, P::BIT_WIDTH as usize, None) {
            found.slide = Some(ns);
            return found;
        }
        if self.len() == self.max_capacity() {
            return found;
        }
        if let Ok(g) = self.grow_and_spread() {
            g.fix_p(&mut pos);
            found.grew = Some(g);
        }
        found.slide = self.store().find_slot(pos, dir, self.len(), None);
        found
    }

    fn grow_and_spread(&mut self) -> Result<GrewFixup, InsufficientMaxCapacity> {
        let shift = self.translator().shift();
        if shift == 0 || self.store().len() * 2 > VecStore::<T, CAP>::max_capacity() {
            return Err(InsufficientMaxCapacity());
        }
        self.translator_mut().set_shift(shift - 1);
        self.store_mut().spread(0);
        Ok(GrewFixup { shl: 1, shift_offset: 0 })
    }
}

impl<'a, T, P, O: Ordering, const CAP: usize, Meta: 'a + Default + Clone> SparseBlock<'a>
    for Block<'a, T, P, VecStore<T, CAP>, FixedRoot<O>, Meta>
where
    T: Sized + 'a,
    P: BlockIndex,
{
    sparse_split!();

    fn find_slot(&mut self, mut pos: usize, dir: bool) -> FoundSlot {
        let mut found = FoundSlot { grew: None, slide: None };
        let shift = self.translator().shift();
        let mut pin = Some(self.v2p(root_vaddr::<O, P>()));
        if self.occupied() * 3 > self.len() * 4 && shift > 0 {
            if let Ok(g) = self.grow_and_spread() {
                g.fix_p(&mut pos);
                if let Some(p) = pin.as_mut() {
                    g.fix_p(p);
                }
                found.grew = Some(g);
            }
        }
        if let Some(ns) = self.store().find_slot(pos, dir, P::BIT_WIDTH as usize, pin) {
            found.slide = Some(ns);
            return found;
        }
        if self.len() == self.max_capacity() {
            return found;
        }
        if let Ok(g) = self.grow_and_spread() {
            g.fix_p(&mut pos);
            if let Some(p) = pin.as_mut() {
                g.fix_p(p);
            }
            found.grew = Some(g);
        }
        found.slide = self.store().find_slot(pos, dir, self.len(), pin);
        found
    }

    ///root is always pinned at `v2p(root_vaddr)` — override the `pin=None` default.
    fn slide_none(&mut self, ms: NoneSlide) -> OpenSlot {
        let pin = Some(self.v2p(root_vaddr::<O, P>()));
        OpenSlot(self.store_mut().slide_none(ms, pin))
    }

    fn grow_and_spread(&mut self) -> Result<GrewFixup, InsufficientMaxCapacity> {
        let shift = self.translator().shift();
        if shift == 0 || self.store().len() * 2 > VecStore::<T, CAP>::max_capacity() {
            return Err(InsufficientMaxCapacity());
        }
        self.translator_mut().set_shift(shift - 1);
        let (spread, shrink_outer) = match O::ROOT_POS {
            RootPos::End => (1usize, true),
            _ => (0usize, false),
        };
        if shrink_outer {
            let tr = self.translator_mut();
            let new_outer = tr.outer_offset() >> 1;
            tr.set_outer_offset(new_outer);
        }
        self.store_mut().spread(spread);
        Ok(GrewFixup { shl: 1, shift_offset: spread as u8 })
    }
}

impl<'a, T, P, const CAP: usize, Meta: 'a + Default + Clone> SparseBlock<'a>
    for Block<'a, T, P, DequeStore<T, CAP>, Pluripotent, Meta>
where
    T: Sized + 'a,
    P: BlockIndex,
{
    sparse_split!();

    fn find_slot(&mut self, mut pos: usize, dir: bool) -> FoundSlot {
        let mut found = FoundSlot { grew: None, slide: None };
        let budget = P::Half::BIT_WIDTH as usize;
        if let Some(ns) = self.store().find_slot(pos, dir, budget, None) {
            found.slide = Some(ns);
            return found;
        }
        if self.len() == self.max_capacity() {
            return found;
        }
        if let Ok(g) = self.grow_and_spread() {
            g.fix_p(&mut pos);
            found.grew = Some(g);
        }
        found.slide = self.store().find_slot(pos, dir, self.len(), None);
        found
    }

    fn grow_and_spread(&mut self) -> Result<GrewFixup, InsufficientMaxCapacity> {
        let shift = self.translator().shift();
        if shift == 0 || self.store().len() * 2 > DequeStore::<T, CAP>::max_capacity() {
            return Err(InsufficientMaxCapacity());
        }
        self.translator_mut().set_shift(shift - 1);
        self.store_mut().spread(0);
        Ok(GrewFixup { shl: 1, shift_offset: 0 })
    }
}

// ---------------------------------------------------------------------------
// per-usecase type aliases. each is a concrete `Block<...,Mode,...>`; `new`/`iter` are
// generic on `Block` (above). push-capable modes add inherent `try_push_*` below.
// ---------------------------------------------------------------------------

///no-pin full-range sparse block.
pub type UniformBlock<'a, T, P, const CAP: usize, Meta = ()> =
    Block<'a, T, P, VecStore<T, CAP>, Uniform, Meta>;
///root pinned at a fixed vaddr (preorder=0, inorder=MIDPOINT, postorder=MAX).
pub type FixedRootBlock<'a, T, P, O, const CAP: usize, Meta = ()> =
    Block<'a, T, P, VecStore<T, CAP>, FixedRoot<O>, Meta>;
///half-range, both-ends-growable, root slides. `DequeStore`.
pub type PluripotentBlock<'a, T, P, const CAP: usize, Meta = ()> =
    Block<'a, T, P, DequeStore<T, CAP>, Pluripotent, Meta>;
///dense `push_back` only (with a periodic None gap for mid-inserts).
pub type AppendBlock<'a, T, P, const CAP: usize, Meta = ()> =
    Block<'a, T, P, VecStore<T, CAP>, Append, Meta>;
///`Append` mirrored: dense `push_front` only, iteration high→low.
pub type PrependBlock<'a, T, P, const CAP: usize, Meta = ()> =
    Block<'a, T, P, VecStore<T, CAP>, Prepend, Meta>;

// --- push-capable modes: inherent push surface (not a trait). ---

impl<'a, T, P, const CAP: usize, Meta: 'a + Default + Clone> Block<'a, T, P, DequeStore<T, CAP>, Pluripotent, Meta>
where
    T: Sized + 'a,
    P: BlockIndex,
{
    pub fn try_push_back(&mut self, v: T) -> Result<usize, T> {
        if self.store.len() < CAP {
            return Ok(self.store.push_back(v));
        }
        Err(v)
    }
    pub fn try_push_front(&mut self, v: T) -> Result<usize, T> {
        if self.store.len() < CAP {
            //outer -= 1<<shift lowers vaddr by the same amount without inner overflow.
            self.store.push_front(v);
            let sh = self.translator.shift();
            let new_outer = self
                .translator
                .outer_offset()
                .wrapping_sub(P::from_usize(1usize << sh));
            self.translator.set_outer_offset(new_outer);
            return Ok(0);
        }
        Err(v)
    }
}

impl<'a, T, P, const CAP: usize, Meta: 'a + Default + Clone> Block<'a, T, P, VecStore<T, CAP>, Append, Meta>
where
    T: Sized + 'a,
    P: BlockIndex,
{
    ///dense push_back; every 16th push stocks a None gap for mid-inserts.
    pub fn try_push_back(&mut self, v: T) -> Result<usize, T> {
        let occ = self.store.occupied();
        let pad = occ != 0 && occ % 16 == 0;
        if self.store.len() + 1 + pad as usize > CAP {
            return Err(v);
        }
        if pad {
            self.store.grow_back(1);
        }
        Ok(self.store.push_back(v))
    }
}

impl<'a, T, P, const CAP: usize, Meta: 'a + Default + Clone> Block<'a, T, P, VecStore<T, CAP>, Prepend, Meta>
where
    T: Sized + 'a,
    P: BlockIndex,
{
    ///dense push_front (= physical push_back, front=high); every 16th push stocks a None gap.
    pub fn try_push_front(&mut self, v: T) -> Result<usize, T> {
        let occ = self.store.occupied();
        let pad = occ != 0 && occ % 16 == 0;
        if self.store.len() + 1 + pad as usize > CAP {
            return Err(v);
        }
        if pad {
            self.store.grow_back(1);
        }
        Ok(self.store.push_back(v))
    }
}

///tree block marker: a `Uniform` block carrying an `Ordering` `O`. adds no methods — it
/// exists so the walker can bound `B: TreeBlock<O>` and link its ordering to the block's.
/// the block itself is O-agnostic storage; per-O behavior (incl. the tree split) lives in
/// the walker, not here.
pub trait TreeBlock<'a, O: Ordering>: SparseBlock<'a> + BlockCursorOf<'a> {}

impl<'a, O: Ordering, T, P, const CAP: usize, Meta: 'a + Default + Clone> TreeBlock<'a, O>
    for Block<'a, T, P, VecStore<T, CAP>, Uniform, Meta>
where
    T: Sized + 'a,
    P: BlockIndex,
    Block<'a, T, P, VecStore<T, CAP>, Uniform, Meta>: SparseBlock<'a> + BlockCursorOf<'a>,
{
}

#[cfg(test)]
#[path = "tests/block.rs"]
mod tests;
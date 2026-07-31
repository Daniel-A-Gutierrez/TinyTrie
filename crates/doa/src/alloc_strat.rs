use std::marker::PhantomData;

use crate::{InOrder, Ordering, PostOrder, PreOrder, block::RawBlock, index::*, store::{DequeStore, VecStore}, translator::Translator};
pub trait AllocStrat<P: BlockIndex>: 'static {
    ///initial shift for an empty block. Uniform = P::BIT_WIDTH (full range);
    ///Pluripotent = P::Half::BIT_WIDTH; Append/Prepend = 0 (dense).
    const INIT_SHIFT: u32;
    ///initial capacity for a block with this strategy. generally 1 or 2. 
    const INIT_CAP: u32;
    ///initial offset (as usize; new_block wraps into P). Anchor so growth has
    ///headroom on the non-dominant side.
    const INIT_INNER_OFFSET: usize;
    const INIT_OUTER_OFFSET: usize;
    ///spread stride: element at i lands at 2i + SPREAD_OFFSET (0 = evens, 1 = odds).
    const SPREAD_OFFSET: usize;
    ///on_grow offset deltas (see doa.md): InOrder inner<<=1, PostOrder outer>>=1,
    ///PreOrder/Pluripotent/Append/Prepend neither.
    const INNER_OFFSET_GROWS: bool;
    const OUTER_OFFSET_SHRINKS: bool;
    ///budget for the find_slot walk before triggering a spread.    
    const INSERT_BUDGET: usize;
    ///max legal store CAP.
    const CAP_LIMIT: usize;
    ///logical direction reversed (front = high end). Prepend only.
    const REVERSED: bool;

    ///translator mutations per event.
    fn on_grow(t: &mut Translator<P>) {
        t.set_shift(t.shift() - 1);
        if Self::INNER_OFFSET_GROWS {
            t.set_inner_offset(t.inner_offset() << 1);
        }
        if Self::OUTER_OFFSET_SHRINKS {
            t.set_outer_offset(t.outer_offset() >> 1);
        }
    }
    fn on_push_front(_t: &mut Translator<P>) {}
    fn on_push_back(_t: &mut Translator<P>) {}

    //TODO on_split -> (left translator, right translator)
}

pub struct Uniform<O : Ordering> ( PhantomData<O> );
pub struct Pluripotent<O : Ordering> ( PhantomData<O> );
pub struct Append;
pub struct Prepend;

///emits the on_push_front override only when a dense prepend bumps inner_offset.
macro_rules! strat_push_front {
    (false) => {};
    (true) => {
        fn on_push_front(t: &mut Translator<P>) {
            t.set_inner_offset(t.inner_offset().wrapping_add(P::ONE));
        }
    };
}

///one AllocStrat impl per row. `$g` is the impl generics (`P: BlockIndex`, plus
///`O: Ordering` for Pluripotent). push_front_grows: false (Uniform — no-op) / true
///(Pluripotent/Append/Prepend — inner_offset += 1 cancels a physical push_front's shift).
macro_rules! strat {
    (
        ($($g:tt)*) => $ty:ty,
        { shift: $shift:expr, cap: $cap:expr,
           inner: $inner:expr, outer: $outer:expr,
           spread: $spread:expr, inner_grows: $ig:expr, outer_shrinks: $os:expr,
           budget: $budget:expr, cap_limit: $clim:expr, reversed: $rev:expr,
           push_front_grows: $pfg:tt, }
    ) => {
        impl<$($g)*> AllocStrat<P> for $ty {
            const INIT_SHIFT: u32 = $shift;
            const INIT_CAP: u32 = $cap;
            const INIT_INNER_OFFSET: usize = $inner;
            const INIT_OUTER_OFFSET: usize = $outer;
            const SPREAD_OFFSET: usize = $spread;
            const INNER_OFFSET_GROWS: bool = $ig;
            const OUTER_OFFSET_SHRINKS: bool = $os;
            const INSERT_BUDGET: usize = $budget;
            const CAP_LIMIT: usize = $clim;
            const REVERSED: bool = $rev;
            strat_push_front!($pfg);
        }
    };
}

strat!((P: BlockIndex) => Uniform<PreOrder>, {
    shift: P::BIT_WIDTH as u32, cap: 1,
    inner: 0, outer: 0, spread: 0,
    inner_grows: false, outer_shrinks: false,
    budget: P::BIT_WIDTH as usize, cap_limit: 1 << P::BIT_WIDTH, reversed: false,
    push_front_grows: false,
});

strat!((P: BlockIndex) => Uniform<InOrder>, {
    shift: P::BIT_WIDTH as u32 - 1, cap: 2,
    inner: 0, outer: 0, spread: 0,
    inner_grows: false, outer_shrinks: false,
    budget: P::BIT_WIDTH as usize, cap_limit: 1 << P::BIT_WIDTH, reversed: false,
    push_front_grows: false,
});

strat!((P: BlockIndex) => Uniform<PostOrder>, {
    shift: P::BIT_WIDTH as u32, cap: 1,
    inner: 1 << (P::BIT_WIDTH - 1), outer: 0, spread: 1,
    inner_grows: false, outer_shrinks: true,
    budget: P::BIT_WIDTH as usize, cap_limit: 1 << P::BIT_WIDTH, reversed: false,
    push_front_grows: false,
});

strat!((P: BlockIndex, O: Ordering) => Pluripotent<O>, {
    shift: P::Half::BIT_WIDTH as u32 - 1, cap: 1,
    inner: 1 << (P::BIT_WIDTH - 1), outer: 0, spread: 0,
    inner_grows: false, outer_shrinks: false,
    budget: P::Half::BIT_WIDTH as usize, cap_limit: 1 << P::Half::BIT_WIDTH, reversed: false,
    push_front_grows: true,
});

strat!((P: BlockIndex) => Append, {
    shift: 0, cap: 1,
    inner: (1 << P::BIT_WIDTH) - (1 << P::Half::BIT_WIDTH), outer: 0, spread: 0,
    inner_grows: false, outer_shrinks: false,
    budget: 16, cap_limit: (1 << P::BIT_WIDTH) - (1 << P::Half::BIT_WIDTH), reversed: false,
    push_front_grows: true,
});

strat!((P: BlockIndex) => Prepend, {
    shift: 0, cap: 1,
    inner: (1 << P::BIT_WIDTH) - (1 << P::Half::BIT_WIDTH), outer: 0, spread: 0,
    inner_grows: false, outer_shrinks: false,
    budget: 16, cap_limit: (1 << P::BIT_WIDTH) - (1 << P::Half::BIT_WIDTH), reversed: true,
    push_front_grows: true,
});
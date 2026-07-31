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

///emits the on_push_front override for Append/Prepend (shift=0): inner_offset -= 1
///lowers vaddr by 1<<shift (=1) and walks inner down to MIN (the exhaustion sentinel).
///Pluripotent overrides on_push_front separately (outer -= 1<<shift; see below).
macro_rules! strat_push_front {
    (false) => {};
    (true) => {
        fn on_push_front(t: &mut Translator<P>) {
            t.set_inner_offset(t.inner_offset().wrapping_sub(P::ONE));
        }
    };
}

///one AllocStrat impl per row. `$g` is the impl generics (`P: BlockIndex`, plus
///`O: Ordering` for Pluripotent). push_front_grows: false (Uniform — no-op) / true
///(Pluripotent/Append/Prepend — inner_offset -= 1 cancels a physical push_front's
///vaddr shift: inner is pre-shift physical space, so -=1 lowers vaddr by 1<<shift).
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

// Pluripotent written by hand (not via strat!) because its on_push_front lives in
// OUTER (virtual) space, not inner: shift>0 means (p+inner)<<shift overflows when
// inner goes negative (wrapping), breaking v2p round-trip on the new front slot.
// outer -= 1<<shift lowers vaddr by the same amount without overflow; inner stays 0
// so all slots stay canonical.
impl<P: BlockIndex, O: Ordering> AllocStrat<P> for Pluripotent<O> {
    const INIT_SHIFT: u32 = P::Half::BIT_WIDTH as u32 - 1;
    const INIT_CAP: u32 = 1;
    const INIT_INNER_OFFSET: usize = 0;
    const INIT_OUTER_OFFSET: usize = 1 << (P::BIT_WIDTH - 1);
    const SPREAD_OFFSET: usize = 0;
    const INNER_OFFSET_GROWS: bool = false;
    const OUTER_OFFSET_SHRINKS: bool = false;
    const INSERT_BUDGET: usize = P::Half::BIT_WIDTH as usize;
    const CAP_LIMIT: usize = 1 << P::Half::BIT_WIDTH;
    const REVERSED: bool = false;
    fn on_push_front(t: &mut Translator<P>) {
        t.set_outer_offset(t.outer_offset().wrapping_sub(P::from_usize(1 << t.shift())));
    }
}

strat!((P: BlockIndex) => Append, {
    shift: 0, cap: 1,
    inner: 1 << P::Half::BIT_WIDTH, outer: 0, spread: 0,
    inner_grows: false, outer_shrinks: false,
    budget: 16, cap_limit: (1 << P::BIT_WIDTH) - (1 << P::Half::BIT_WIDTH), reversed: false,
    push_front_grows: true,
});

strat!((P: BlockIndex) => Prepend, {
    shift: 0, cap: 1,
    inner: 1 << P::Half::BIT_WIDTH, outer: 0, spread: 0,
    inner_grows: false, outer_shrinks: false,
    budget: 16, cap_limit: (1 << P::BIT_WIDTH) - (1 << P::Half::BIT_WIDTH), reversed: true,
    push_front_grows: true,
});
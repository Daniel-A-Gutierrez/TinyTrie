//!module wiring + the ordering + relative-side vocabulary. `Ordering` impls are
//!how a block names its traversal order so tree ops can `match O::ORDER` and
//!monomorphize per-ordering flows. re-exports `metadata::Fixup`.

pub use metadata::Fixup;

pub mod blocks;
pub mod index;
pub mod metadata;
pub mod store;
pub mod translator;
pub mod treeblock;
pub mod walker;
//unwired for the addr/pos terminology refactor — port when they earn a consumer:
//mod inline_leafblock;
//mod leafblock;

pub struct InOrder;
pub struct PreOrder;
pub struct PostOrder;

///where the tree root lives in a fresh block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootPos {
    Beginning,
    Middle,
    End,
}

///which ordering a block uses. a const so tree ops can `match` on it and
///monomorphize into a per-ordering flow that differs in *steps* (splits), not just
///values (`suggest_*` methods cover those).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    Pre,
    In,
    Post,
}

///which side of an anchor a slot opens on (or a suggestion names).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rel {
    Before,
    After,
}

///impl'd by `InOrder` (Middle/In), `PreOrder` (Beginning/Pre), `PostOrder` (End/Post).
pub trait Ordering: 'static {
    const ROOT_POS: RootPos;
    const ORDER: Order;
}

///easiest to split, iteration OK
impl Ordering for InOrder {
    const ROOT_POS: RootPos = RootPos::Middle;
    const ORDER: Order = Order::In;
}
impl Ordering for PreOrder {
    const ROOT_POS: RootPos = RootPos::Beginning;
    const ORDER: Order = Order::Pre;
}
impl Ordering for PostOrder {
    const ROOT_POS: RootPos = RootPos::End;
    const ORDER: Order = Order::Post;
}

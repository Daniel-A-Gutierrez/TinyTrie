pub mod index;
mod inline_leafblock;
mod leafblock;
pub mod blocks;
pub mod metadata;
pub mod treeblock;
pub mod store;
pub mod translator;
pub mod walker;

pub struct InOrder;
pub struct PreOrder;
pub struct PostOrder;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootPos { Beginning, Middle, End }

///which tree ordering a block uses. a const so tree ops can `match` on it and
///monomorphize into a per-ordering flow that differs in *steps* (splits), not just
///values (`suggest_*` methods cover those).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order { Pre, In, Post }

///where the tree root lives in a fresh block.
pub trait Ordering: 'static {
    const ROOT_POS: RootPos;
    const ORDER: Order;
}

///easiest to split, iteration OK
impl Ordering for InOrder   { const ROOT_POS: RootPos = RootPos::Middle; const ORDER: Order = Order::In; }
impl Ordering for PreOrder  { const ROOT_POS: RootPos = RootPos::Beginning; const ORDER: Order = Order::Pre; }
impl Ordering for PostOrder { const ROOT_POS: RootPos = RootPos::End; const ORDER: Order = Order::Post; }

pub use metadata::Fixup;
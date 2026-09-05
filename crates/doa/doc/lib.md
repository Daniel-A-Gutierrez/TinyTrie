```rust
//!module wiring + the ordering vocabulary. `Ordering` impls are how a block
//!names its traversal order so tree ops can `match O::ORDER` and monomorphize
//!per-ordering flows. re-exports `metadata::Fixup`.
///L0005
pub use metadata::Fixup;
///L0007
pub mod blocks;
///L0008
pub mod index;
///L0009
mod inline_leafblock;
///L0010
mod leafblock;
///L0011
pub mod metadata;
///L0012
pub mod store;
///L0013
pub mod translator;
///L0014
pub mod treeblock;
///L0015
pub mod walker;
///L0017
pub struct InOrder;
///L0018
pub struct PreOrder;
///L0019
pub struct PostOrder;
///L0023
///where the tree root lives in a fresh block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootPos {
    Beginning,
    Middle,
    End,
}
///L0033
///which ordering a block uses. a const so tree ops can `match` on it and
///monomorphize into a per-ordering flow that differs in *steps* (splits), not just
///values (`suggest_*` methods cover those).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    Pre,
    In,
    Post,
}
///L0040
///impl'd by `InOrder` (Middle/In), `PreOrder` (Beginning/Pre), `PostOrder` (End/Post).
pub trait Ordering: 'static {
    const ROOT_POS: RootPos;
    const ORDER: Order;
}
///L0046
///easiest to split, iteration OK
impl Ordering for InOrder {}
///L0050
impl Ordering for PreOrder {}
///L0054
impl Ordering for PostOrder {}
```

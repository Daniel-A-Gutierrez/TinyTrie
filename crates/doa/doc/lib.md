```rust
//!module wiring + the ordering + relative-side vocabulary. `Ordering` impls are
//!how a block names its traversal order so tree ops can `match O::ORDER` and
//!monomorphize per-ordering flows. re-exports `metadata::Fixup`.
///L0005
pub use metadata::Fixup;
///L0007
pub mod blocks;
///L0008
pub mod index;
///L0009
pub mod metadata;
///L0010
pub mod store;
///L0011
pub mod translator;
///L0012
pub mod treeblock;
///L0013
pub mod walker;
//unwired for the addr/pos terminology refactor — port when they earn a consumer:
//mod inline_leafblock;
//mod leafblock;
///L0018
pub struct InOrder;
///L0019
pub struct PreOrder;
///L0020
pub struct PostOrder;
///L0024
///where the tree root lives in a fresh block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootPos {
    Beginning,
    Middle,
    End,
}
///L0034
///which ordering a block uses. a const so tree ops can `match` on it and
///monomorphize into a per-ordering flow that differs in *steps* (splits), not just
///values (`suggest_*` methods cover those).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    Pre,
    In,
    Post,
}
///L0042
///which side of an anchor a slot opens on (or a suggestion names).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rel {
    Before,
    After,
}
///L0048
///impl'd by `InOrder` (Middle/In), `PreOrder` (Beginning/Pre), `PostOrder` (End/Post).
pub trait Ordering: 'static {
    const ROOT_POS: RootPos;
    const ORDER: Order;
}
///L0054
///easiest to split, iteration OK
impl Ordering for InOrder {}
///L0058
impl Ordering for PreOrder {}
///L0062
impl Ordering for PostOrder {}
```

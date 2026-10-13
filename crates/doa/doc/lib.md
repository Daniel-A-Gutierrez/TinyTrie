```rust
//!module wiring + the ordering + relative-side vocabulary. `Ordering` impls are
//!how a block names its traversal order so tree ops can `match O::ORDER` and
//!monomorphize per-ordering flows. re-exports `treeblock::metadata::Fixup`.
//!
//!two consumers over the shared substrate (`index`/`translator`):
//!`treeblock` (the walker/tree machinery + its store/fixup protocol) and
//!`sortedblock` (the set — new, pairs-based feedback).
///L0009
pub use treeblock::metadata::Fixup;
///L0011
pub mod index;
///L0012
pub mod mask;
///L0013
pub mod sortedblock;
///L0014
pub mod translator;
///L0015
pub mod treeblock;
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
///L0041
///which side of an anchor a slot opens on (or a suggestion names).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rel {
    Before,
    After,
}
///L0047
///impl'd by `InOrder` (Middle/In), `PreOrder` (Beginning/Pre), `PostOrder` (End/Post).
pub trait Ordering: 'static {
    const ROOT_POS: RootPos;
    const ORDER: Order;
}
///L0053
///easiest to split, iteration OK
impl Ordering for InOrder {}
///L0057
impl Ordering for PreOrder {}
///L0061
impl Ordering for PostOrder {}
```

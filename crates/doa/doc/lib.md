```rust
//!module wiring + the ordering + relative-side vocabulary. `Ordering` impls are
//!how a block names its traversal order so tree ops can `match O::ORDER` and
//!monomorphize per-ordering flows. re-exports `treeblock::metadata::Fixup`.
//!
//!the set/map tiers are the primary surface, flat in src/: `blockset`/`blockmap`/`blockbimap`
//!over the `index`/`mask`/`translator` (2-knob) substrate; `treeblock` (the
//!walker/tree machinery + its store/fixup protocol + its own 4-knob
//!translator) holds the dense-arena machinery.
///L0010
pub use treeblock::metadata::Fixup;
///L0012
pub mod blockbimap;
///L0013
pub mod blockmap;
///L0014
pub mod blockset;
///L0015
pub mod index;
///L0016
pub mod mask;
///L0017
pub mod serialize;
///L0018
pub mod skipset;
///L0019
pub mod translator;
///L0020
pub mod treeblock;
///L0022
pub struct InOrder;
///L0023
pub struct PreOrder;
///L0024
pub struct PostOrder;
///L0028
///where the tree root lives in a fresh block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootPos {
    Beginning,
    Middle,
    End,
}
///L0038
///which ordering a block uses. a const so tree ops can `match` on it and
///monomorphize into a per-ordering flow that differs in *steps* (splits), not just
///values (`suggest_*` methods cover those).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    Pre,
    In,
    Post,
}
///L0046
///which side of an anchor a slot opens on (or a suggestion names).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rel {
    Before,
    After,
}
///L0052
///impl'd by `InOrder` (Middle/In), `PreOrder` (Beginning/Pre), `PostOrder` (End/Post).
pub trait Ordering: 'static {
    const ROOT_POS: RootPos;
    const ORDER: Order;
}
///L0058
///easiest to split, iteration OK
impl Ordering for InOrder {}
///L0062
impl Ordering for PreOrder {}
///L0066
impl Ordering for PostOrder {}
```

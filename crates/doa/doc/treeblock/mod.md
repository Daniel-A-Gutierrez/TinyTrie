```rust
//!the tree consumer: `Block` + per-mode `BlockOps` (blocks.rs), the walker
//!layers (walker.rs), the `TreeBlock` surface + constructors (treeblock.rs),
//!the fixup/state vocabulary (metadata.rs). unwired: block_cursor/leafblock/
//!inline_leafblock (pre-refactor surfaces — port when they earn a consumer).
///L0006
pub mod blocks;
///L0007
pub mod metadata;
///L0008
pub mod store;
///L0009
pub mod translator;
///L0010
pub mod walker;
//unwired for the addr/pos terminology refactor — port when they earn a consumer:
//mod inline_leafblock;
//mod leafblock;
///L0016
mod treeblock;
///L0017
pub use treeblock::{TreeBlock, search, walker};
```

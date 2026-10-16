//!the tree consumer: `Block` + per-mode `BlockOps` (blocks.rs), the walker
//!layers (walker.rs), the `TreeBlock` surface + constructors (treeblock.rs),
//!the fixup/state vocabulary (metadata.rs). unwired: block_cursor/leafblock/
//!inline_leafblock (pre-refactor surfaces — port when they earn a consumer).

pub mod blocks;
pub mod metadata;
pub mod store;
pub mod translator;
pub mod walker;

//unwired for the addr/pos terminology refactor — port when they earn a consumer:
//mod inline_leafblock;
//mod leafblock;

mod treeblock;
pub use treeblock::{TreeBlock, search, walker};

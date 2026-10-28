//!fixed-storage dual-front allocator: `Block<P>` = header + trailing data in
//!ONE allocation; `[stack | free | heap]` grow toward each other from the data
//!region's ends, so every op is O(1) arithmetic on two lengths.
//!`S<P>` names stack bytes (front-relative), `H<P>` heap bytes (back-relative) —
//!distinct types so neither is accepted where the other is expected; both
//!survive `grow`'s relocation. the serialized form is the raw allocation bytes
//!(`Header` = 16 bytes, `ptr` = 16, cap rounded to 16 — fixed-cap blocks are
//!fixed-stride on disk). tasks 1–2 of the block effort: the allocator core
//!plus Archive/RelOrd over its seams.

mod archive;
mod block;
mod handle;
mod stack;
mod stacks;

pub use archive::{Archive, Heap, RelOrd, StrHandle, VecH};
pub use block::{Allocator, Block, CapErr, MAX_ALIGN};
pub use handle::{H, Pad, Ptr, S};
pub use stack::{LoadErr, RECORD, Stack};
pub use stacks::{Lease, Stacks, StacksErr, DEFAULT_BUDGET};
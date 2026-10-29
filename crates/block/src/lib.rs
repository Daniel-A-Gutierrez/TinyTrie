//!fixed-storage dual-front allocator: `Block` = header + trailing data in
//!ONE allocation; `[stack | free | heap]` grow toward each other from the
//!data region's ends, so every op is O(1) arithmetic on two lengths.
//!`S<T>` names stack bytes (front-relative), `H<T>` heap bytes
//!(back-relative) — typed, unforgeable handles; both survive `grow`'s
//!relocation. u16-pointer world: cap ≤ 65520, the freelist head owns the
//!data region's last NODE bytes. the serialized form is the raw allocation
//!bytes (`Header` = 16 bytes, `ptr` = 16, cap rounded to 16 — fixed-cap
//!blocks are fixed-stride on disk). tasks 1–2 of the block effort: the
//!allocator core plus Archive/RelOrd over its seams.

mod archive;
mod block;
mod handle;
mod stack;
mod stacks;

pub use archive::{Archive, Heap, RelOrd, StrHandle, VecHandle};
pub use block::{Allocator, Block, CapErr, MAX_ALIGN};
pub use handle::{H, Pad, ReprC, S, ZeroPad};
pub use stack::{LoadErr, RECORD, Stack};
pub use stacks::{DEFAULT_BUDGET, Lease, Stacks, StacksErr};

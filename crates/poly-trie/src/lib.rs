#![feature(portable_simd)]

//! Graduated radix trie with adaptive node sizes (Node2/Node4/Node16).
//!
//! Nodes start as binary (Node2) and graduate to 4-way and 16-way as branching
//! increases. Uses an arena allocator with block-size free lists for efficient
//! node allocation.
//!
//! # Null-Terminator Contract
//!
//! `insert()` rejects keys containing `0x00` and appends a null terminator
//! internally. `get()` requires null-terminated input.

mod arena;
mod poly_trie;

pub use arena::Arena;
pub use poly_trie::{NodeRef, PolyTrie};
pub use benchable_map::BenchableMap;


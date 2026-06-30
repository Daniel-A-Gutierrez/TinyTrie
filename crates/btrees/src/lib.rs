#![allow(incomplete_features)]
#![feature(generic_const_exprs)]
#![feature(portable_simd)]
#![allow(internal_features)]
#![feature(nonzero_internals)]

//! Compact B+ tree with SIMD-accelerated node search.
//!
//! CTree is an ordered map backed by a B+ tree with configurable node size (`N`)
//! and SIMD-accelerated lower-bound search. Variable-length keys can use a
//! fixed-size preview (`P`) for fast node-level routing.

pub mod key_slots;
pub mod tiny_array;
pub mod int_btree;
pub mod str_btree;

pub use int_btree::{
    BufKey, CTree, Cursor, CursorMut, FixedLenKey, KeyRef,
    SearchStrategy, StoredKey, TreeKey, TrieIndex,
};
pub use tiny_array::TinyArray;
pub use str_btree::{StrBTree, StrBTreeKey, LengthType};
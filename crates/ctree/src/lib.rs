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

pub mod packed_keys;
pub mod tiny_array;
pub mod tiny_btree;
pub mod var_btree;

pub use tiny_btree::{
    BufKey, CTree, Cursor, CursorMut, FixedCTree, FixedLenKey, KeyRef,
    SearchStrategy, StoredKey, TreeKey, TrieIndex, VarCTree,
};
pub use tiny_array::TinyArray;
pub use var_btree::{VarCTree as PackedVarCTree, VarKey as PackedVarKey, TrieIndex as PackedTrieIndex, LengthType as PackedLengthType};


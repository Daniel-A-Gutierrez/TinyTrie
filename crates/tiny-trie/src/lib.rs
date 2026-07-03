#![allow(incomplete_features)]
#![feature(generic_const_exprs)]
#![feature(portable_simd)]

//! Compact arena-based radix tries with SIMD-accelerated lookup.
//!
//! This crate provides several trie data structures optimized for different use cases:
//!
//! - [`NibbleTrie`] — 16-way radix trie (nibble-indexed), the flagship implementation
//! - [`NibTrie`] — 4-way radix trie (2-bit indexed)
//! - [`BitTrie`] — Binary radix trie (bit-indexed)
//! - [`DynTrie`] — Auto-promoting wrapper around NibbleTrie
//! - [`FixedLenNibbleTrie`] — NibbleTrie variant for fixed-length keys
//!
//! All tries use arena-based node allocation for cache-friendly memory layout and
//! SIMD-accelerated child lookup where applicable.

mod simd;
pub mod nibble_trie;
pub mod nib_trie;
pub mod bit_trie;
pub mod dyn_trie;
pub mod fixed_len_nibble_trie;
mod key_store;
mod tiny_array;
pub mod flat_node;

pub use tiny_trie_trait::TinyTrieMap;

pub use nibble_trie::{NibbleTrie, Node, OptNz, TrieIndex};
pub use nib_trie::{NibTrie, NibNode};
pub use bit_trie::BitTrie;
pub use dyn_trie::DynTrie;
pub use fixed_len_nibble_trie::{FixedLenNibbleTrie, FixedLenNode};
pub use key_store::{ByteKey, BufKeyStore, KeyStore, NonNullKey, NonZeroBytes, TrieKey, U64Key, VecKeyStore};

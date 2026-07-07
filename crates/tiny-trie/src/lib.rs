#![feature(portable_simd)]

//! Compact arena-based radix tries with SIMD-accelerated lookup.
//!
//! This crate provides several trie data structures optimized for different use cases:
//!
//! - [`NibbleTrie`] — 16-way radix trie (nibble-indexed), the flagship implementation
//! - [`NibTrie`] — 4-way radix trie (2-bit indexed)
//! - [`BitTrie`] — Binary radix trie (bit-indexed)
//! - [`DynTrie`] — Auto-promoting wrapper around NibbleTrie (requires the `dyn` feature)
//! - [`FixedLenNibbleTrie`] — NibbleTrie variant for fixed-length keys (requires the
//!   `fixed-len` feature)
//!
//! All tries use arena-based node allocation for cache-friendly memory layout and
//! SIMD-accelerated child lookup where applicable.
//!
//! Each tree exposes a seekable cursor over `(&[u8], &T)` pairs via its `iter()` /
//! `iter_last()` methods (`nibble_trie::Cursor`, `nib_trie::Cursor`, `bit_trie::Cursor`).

mod simd;
pub mod nibble_trie;
pub mod nib_trie;
pub mod bit_trie;
#[cfg(feature = "dyn")]
pub mod dyn_trie;
#[cfg(feature = "fixed-len")]
pub mod fixed_len_nibble_trie;
mod key_store;
mod tiny_array;

// The three public trees.
pub use nibble_trie::{NibbleTrie, TrieIndex};
pub use nib_trie::NibTrie;
pub use bit_trie::BitTrie;

// Key trait bounds and the non-zero key type used by BitTrie / null-terminator tries.
pub use key_store::{ByteKey, TrieKey};

// Internal-only re-export: `KeyStore` is the bound on `TrieKey::Store` and is used
// by tree modules to call store methods (`push`/`key_bytes`/`rollback`/`len`). Not
// part of the public surface.
pub(crate) use key_store::KeyStore;

// Optional, non-default trees gated behind cargo features.
#[cfg(feature = "dyn")]
pub use dyn_trie::DynTrie;
#[cfg(feature = "fixed-len")]
pub use fixed_len_nibble_trie::FixedLenNibbleTrie;
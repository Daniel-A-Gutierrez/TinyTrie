//! Shared benchmark infrastructure for trie data structures.
//!
//! This crate provides key generation, corpus loading, and result
//! persistence used by both the `bencher` and `trie-stats` binaries.

pub mod keygen;
pub mod results;

// Re-export types that both binaries need.
pub use tiny_trie::NonZeroBytes;
pub use tiny_trie_trait::TinyTrieMap;

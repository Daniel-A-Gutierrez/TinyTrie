//! This crate provides key generation, corpus loading, and result
//! persistence used by both the `bencher` and `trie-stats` binaries.

pub mod benchable_map;
pub mod keygen;
pub mod results;

// Re-export types that both binaries need.
pub use benchable_map::{BenchableMap, NonZeroBytes};

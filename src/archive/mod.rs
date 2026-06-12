mod prefix_len;
mod pairvec;
mod simd;
mod tiny_trie_map;
mod arena;
mod arena_trie;
mod bit_trie;
mod poly_trie;
pub(crate) mod prefix_trie;

// Re-exports so archive-internal `use super::...` paths resolve cleanly.
// (Archive files use `super::` instead of `crate::` to reference each other.)
pub use prefix_len::PrefixLen;
pub use tiny_trie_map::TinyTrieMap;
pub use arena::Arena;
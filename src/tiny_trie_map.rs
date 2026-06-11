/// Unified trait for trie data structures used in benchmarking.
///
/// The `trie_` prefix avoids collisions with inherent methods (e.g. `insert`,
/// `get`, `iter`). Methods are specialized for `usize` values — the bench
/// only stores `usize`, so a generic `T` parameter would add unnecessary
/// complexity.
///
/// Iterator methods use callbacks (`trie_iter_fwd`, `trie_iter_rev`) instead
/// of returning named iterator types, since each trie has its own iterator
/// type and the trait's purpose is abstraction for the bench.
///
/// **Iterator semantics**: `iter()` positions TinyTrie AT the first key
/// (where `current()` works immediately) and the other tries BEFORE the
/// first key (where `current()` returns `None` until `next()` is called).
/// The `trie_iter_fwd`/`trie_iter_rev` implementations handle both cases by
/// calling `current()` first, then looping with `next()`/`prev()`.
pub trait TinyTrieMap: Sized {
    /// Create an empty trie.
    fn trie_new() -> Self;

    /// Insert a key-value pair. For TinyTrie, BitTrie, and PolyTrie, keys
    /// must not contain `0x00`. NibbleTrie accepts any byte including `0x00`.
    fn trie_insert(&mut self, key: Vec<u8>, value: usize);

    /// Look up a key. For TinyTrie, BitTrie, and PolyTrie, the key must be
    /// null-terminated. NibbleTrie accepts plain `&[u8]` keys.
    fn trie_get(&self, key: &[u8]) -> Option<usize>;

    /// Iterate all key-value pairs in forward (ascending) order.
    fn trie_iter_fwd(&self, f: impl FnMut(&[u8], &usize));

    /// Iterate all key-value pairs in reverse (descending) order.
    fn trie_iter_rev(&self, f: impl FnMut(&[u8], &usize));

    /// Iterate all key indices in forward (ascending) order.
    /// Only NibbleTrie implements this — index-only iteration skips key/value reads.
    /// Default: panic (not all tries support index-only iteration).
    fn trie_iter_fwd_index(&self, _f: impl FnMut(usize)) {
        unimplemented!("index-only iteration not supported for this trie type")
    }

    /// Iterate all key indices in reverse (descending) order.
    /// Only NibbleTrie implements this — index-only iteration skips key/value reads.
    /// Default: panic (not all tries support index-only iteration).
    fn trie_iter_rev_index(&self, _f: impl FnMut(usize)) {
        unimplemented!("index-only iteration not supported for this trie type")
    }

    /// Number of entries in the trie.
    fn trie_len(&self) -> usize;

    /// Optimize the trie's memory layout for cache locality.
    /// Default no-op — only NibbleTrie and PolyTrie override this.
    fn trie_optimize(&mut self) {}
}
/// Unified trait for trie data structures used in benchmarking.
///
/// The `map_` prefix avoids collisions with inherent methods (e.g. `insert`,
/// `get`, `iter`). Methods are specialized for `usize` values — the bench
/// only stores `usize`, so a generic `T` parameter would add unnecessary
/// complexity.
///
/// Iterator methods use callbacks (`map_iter_fwd`, `map_iter_rev`) instead
/// of returning named iterator types, since each trie has its own iterator
/// type and the trait's purpose is abstraction for the bench.
///
/// **Iterator semantics**: `iter()` positions TinyTrie AT the first key
/// (where `current()` works immediately) and the other tries BEFORE the
/// first key (where `current()` returns `None` until `next()` is called).
/// The `map_iter_fwd`/`map_iter_rev` implementations handle both cases by
/// calling `current()` first, then looping with `next()`/`prev()`.
pub trait BenchableMap: Sized {
    /// Create an empty trie.
    fn map_new() -> Self;

    /// Insert a key-value pair. For TinyTrie, BitTrie, and PolyTrie, keys
    /// must not contain `0x00`. NibbleTrie accepts any byte including `0x00`.
    fn map_insert(&mut self, key: Vec<u8>, value: usize);

    /// Look up a key. For TinyTrie, BitTrie, and PolyTrie, the key must be
    /// null-terminated. NibbleTrie accepts plain `&[u8]` keys.
    fn map_get(&self, key: &[u8]) -> Option<usize>;

    /// Iterate all key-value pairs in forward (ascending) order.
    fn map_iter_fwd(&self, f: impl FnMut(&[u8], &usize));

    /// Iterate all key-value pairs in reverse (descending) order.
    fn map_iter_rev(&self, f: impl FnMut(&[u8], &usize));

    /// Iterate all key indices in forward (ascending) order.
    /// Only NibbleTrie implements this — index-only iteration skips key/value reads.
    /// Default: panic (not all tries support index-only iteration).
    fn map_iter_fwd_index(&self, _f: impl FnMut(usize)) {
        unimplemented!("index-only iteration not supported for this trie type")
    }

    /// Iterate all key indices in reverse (descending) order.
    /// Only NibbleTrie implements this — index-only iteration skips key/value reads.
    /// Default: panic (not all tries support index-only iteration).
    fn map_iter_rev_index(&self, _f: impl FnMut(usize)) {
        unimplemented!("index-only iteration not supported for this trie type")
    }

    /// Number of entries in the trie.
    fn map_len(&self) -> usize;

    /// Optimize the trie's memory layout for cache locality.
    /// Default no-op — only NibbleTrie and PolyTrie override this.
    fn map_optimize(&mut self) {}
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NonZeroBytes(Vec<u8>);

impl NonZeroBytes {
    /// Construct from a byte slice, returning `None` if it contains `0x00`.
    pub fn new(v: Vec<u8>) -> Option<Self> {
        (!v.contains(&0)).then_some(Self(v))
    }

    /// Construct without checking for `0x00`.
    ///
    /// # Safety
    /// The byte string must not contain `0x00`.
    pub unsafe fn new_unchecked(v: Vec<u8>) -> Self {
        Self(v)
    }

    /// Return the byte representation.
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Return the owned byte vector.
    pub fn into_vec(self) -> Vec<u8> {
        self.0
    }

    /// Clone the inner byte vector.
    ///
    /// Provided for compatibility with code that needs an owned `Vec<u8>`
    /// from a borrowed `NonZeroBytes` (e.g. tries that append a null
    /// terminator internally).
    pub fn to_vec(&self) -> Vec<u8> {
        self.0.clone()
    }
}


// `Default` is required because `ByteKey: TrieKey` and `TrieKey: Default`. The
// default value is only ever used as the dummy entry at store index 0 (never
// inserted as a real key); an empty byte vector contains no `0x00`, so the
// no-embedded-null invariant is preserved.
impl Default for NonZeroBytes {
    fn default() -> Self {
        NonZeroBytes(Vec::new())
    }
}

impl std::ops::Deref for NonZeroBytes {
    type Target = [u8];
    #[inline]
    fn deref(&self) -> &[u8] {
        &self.0
    }
}

impl std::borrow::Borrow<[u8]> for NonZeroBytes {
    #[inline]
    fn borrow(&self) -> &[u8] {
        &self.0
    }
}

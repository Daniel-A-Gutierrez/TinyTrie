//! Key storage traits and implementations for generic trie key types.
//!
//! [`TrieKey`] defines how a key type provides its byte representation for trie
//! traversal, and which [`KeyStore`] implementation backs it. Two store backends
//! are provided:
//!
//! - [`BufKeyStore`] — flat buffer storage for `Vec<u8>` keys (cache-friendly,
//!   single contiguous allocation for all key bytes).
//! - [`VecKeyStore`] — `Vec<K>` storage for any `TrieKey` type (e.g. `String`).
//!
//! [`ByteKey`] is a simpler trait for key types that can be converted to and
//! from `&[u8]` while preserving ordering. It is used by [`NibbleTrie`] and
//! other radix-trie structures that manage their own key storage internally.
//!
//! [`NonNullKey`] is a marker trait for [`ByteKey`] types whose byte
//! representation is guaranteed to contain no `0x00` bytes. Tries that use
//! null-byte sentinels (e.g. [`PolyTrie`]) require `K: NonNullKey`.
//!
//! [`NibbleTrie`]: crate::NibbleTrie
//! [`PolyTrie`]: crate::PolyTrie
use benchable_map::NonZeroBytes;
// ---------------------------------------------------------------------------
// TrieKey
// ---------------------------------------------------------------------------

/// A key type that can be stored in a trie.
///
/// Each key type chooses its storage backend via the associated `Store` type.
/// The `as_bytes()` method provides the byte representation used for trie
/// traversal (bit extraction, divergence comparison).
///
/// The `Default` bound is required because both store backends reserve index 0
/// as a dummy entry (so that key index 0 can serve as the "empty" sentinel in
/// node children arrays). `K::default()` becomes the dummy entry in `VecKeyStore`.
pub trait TrieKey: Default {
    /// The storage backend for this key type.
    type Store: KeyStore<Self>;

    /// Return the byte representation of this key for trie traversal.
    fn as_bytes(&self) -> &[u8];
}

// ---------------------------------------------------------------------------
// KeyStore
// ---------------------------------------------------------------------------

/// Storage backend for trie keys.
///
/// Keys are stored with 1-based indices: index 0 is a dummy entry, so real keys
/// start at index 1. This allows 0 to be used as the "empty child" sentinel in
/// node children arrays.
pub trait KeyStore<K>: Default {
    /// Push a new key, returning its 1-based key index.
    fn push(&mut self, key: K) -> u32;

    /// Get the byte representation of the key at 1-based index `ki`.
    fn key_bytes(&self, ki: u32) -> &[u8];

    /// Rollback the last push (called on duplicate-key insertion).
    fn rollback(&mut self);

    /// Number of real keys (excluding the dummy at index 0).
    fn len(&self) -> usize;

    /// Consume the store and return all real keys (skipping the dummy).
    fn into_keys(self) -> Vec<K>;
}

// ---------------------------------------------------------------------------
// BufKeyStore — flat buffer for Vec<u8> keys
// ---------------------------------------------------------------------------

/// Flat-buffer key storage for `Vec<u8>` keys.
///
/// All key bytes are packed into a single contiguous `Vec<u8>`, with a separate
/// index array recording `(offset, length)` per key. This layout is
/// cache-friendly for sequential access during iteration and divergence scans.
///
/// Key byte lengths are stored as `u16`, limiting individual keys to 65535 bytes.
pub struct BufKeyStore {
    buf: Vec<u8>,
    /// (offset into buf, byte length) per key. index[0] = dummy entry.
    index: Vec<(usize, u16)>,
}

impl Default for BufKeyStore {
    fn default() -> Self {
        BufKeyStore {
            buf: Vec::new(),
            index: vec![(0, 0)], // index[0] = dummy entry
        }
    }
}

impl KeyStore<Vec<u8>> for BufKeyStore {
    fn push(&mut self, key: Vec<u8>) -> u32 {
        let ki = self.index.len() as u32;
        let offset = self.buf.len();
        debug_assert!(key.len() <= u16::MAX as usize, "BufKeyStore key length exceeds u16::MAX");
        self.buf.extend_from_slice(&key);
        self.index.push((offset, key.len() as u16));
        ki
    }

    fn key_bytes(&self, ki: u32) -> &[u8] {
        let (off, len) = self.index[ki as usize];
        &self.buf[off..off + len as usize]
    }

    fn rollback(&mut self) {
        let (off, _len) = self.index.pop().unwrap();
        self.buf.truncate(off);
    }

    fn len(&self) -> usize {
        self.index.len() - 1
    }

    fn into_keys(self) -> Vec<Vec<u8>> {
        let buf = self.buf;
        self.index
            .into_iter()
            .skip(1)
            .map(|(off, len)| buf[off..off + len as usize].to_vec())
            .collect()
    }
}

// ---------------------------------------------------------------------------
// VecKeyStore<K> — Vec<K> storage for any TrieKey
// ---------------------------------------------------------------------------

/// Vec-backed key storage for any `TrieKey` type.
///
/// Each key is stored as its own `K` object in a `Vec<K>`. This is simpler than
/// [`BufKeyStore`] but loses the single-allocation cache locality benefit.
/// For fixed-size key types (e.g. `[u8; 4]`, `Ipv4Addr`), keys are stored inline
/// in the Vec and are still contiguous.
pub struct VecKeyStore<K: TrieKey> {
    keys: Vec<K>, // keys[0] = K::default() dummy
}

impl<K: TrieKey> Default for VecKeyStore<K> {
    fn default() -> Self {
        VecKeyStore {
            keys: vec![K::default()], // keys[0] = dummy entry
        }
    }
}

impl<K: TrieKey> KeyStore<K> for VecKeyStore<K> {
    fn push(&mut self, key: K) -> u32 {
        let ki = self.keys.len() as u32;
        self.keys.push(key);
        ki
    }

    fn key_bytes(&self, ki: u32) -> &[u8] {
        self.keys[ki as usize].as_bytes()
    }

    fn rollback(&mut self) {
        self.keys.pop();
    }

    fn len(&self) -> usize {
        self.keys.len() - 1
    }

    fn into_keys(self) -> Vec<K> {
        self.keys.into_iter().skip(1).collect()
    }
}

// ---------------------------------------------------------------------------
// ByteKey — byte-representation trait for generic trie keys
// ---------------------------------------------------------------------------

/// A key type that can be converted to and from a byte slice while preserving
/// ordering.
///
/// Implementations must ensure that byte-order comparison matches the key
/// type's natural ordering: for all `a`, `b`, `a.bytes().cmp(b.bytes())`
/// must equal `a.cmp(b)`.
///
/// The [`from_bytes`] reconstruction is only ever called with byte sequences
/// originally produced by [`bytes`], so implementations may assume valid
/// input. The same applies to [`as_borrowed`]: it is only ever called with
/// bytes that originated from a `bytes()` call of the same key type, so e.g.
/// the `String` impl may assume valid UTF-8 and skip validation.
///
/// `ByteKey` is a subtrait of [`TrieKey`]; the byte representation is also
/// available via [`TrieKey::as_bytes`]. The `bytes` method is the
/// `ByteKey`-specific accessor used by radix tries that manage their own key
/// storage ([`NibbleTrie`], [`NibTrie`]); [`TrieKey::as_bytes`] is used by
/// [`BitTrie`] and the storage backends. Distinct names avoid ambiguity when
/// both traits are in scope.
///
/// [`NibbleTrie`]: crate::NibbleTrie
/// [`NibTrie`]: crate::NibTrie
/// [`BitTrie`]: crate::BitTrie
/// [`from_bytes`]: ByteKey::from_bytes
/// [`as_borrowed`]: ByteKey::as_borrowed
/// [`bytes`]: ByteKey::bytes
pub trait ByteKey: TrieKey {
    /// Borrowed view of the key, constructible from `&[u8]` without allocation.
    ///
    /// This is the natural zero-alloc form handed back by iteration: `Vec<u8>`
    /// → `&'a [u8]`, `String` → `&'a str`, `NonZeroBytes` → `&'a [u8]`. It
    /// satisfies [`AsRef`]`<[u8]>` so callers can recover the raw bytes when
    /// needed. The [`Borrow`] equivalence contract (Eq/Ord/Hash matching `Self`)
    /// is already guaranteed by this trait's byte-order invariant and is not
    /// re-imposed as a bound here; the trie compares raw `buf` bytes directly.
    type Borrowed<'a>: AsRef<[u8]> + 'a
    where
        Self: 'a;

    /// Return the byte representation of this key.
    fn bytes(&self) -> &[u8];

    /// Reconstruct an *owned* key from its byte representation. Allocates.
    ///
    /// `from_bytes(k.bytes())` must produce a value equivalent to `k`. Use this
    /// only when you need an owned `K` (e.g. collecting into a `Vec<K>`); for
    /// iteration over keys already stored in the trie, prefer [`as_borrowed`].
    ///
    /// [`as_borrowed`]: ByteKey::as_borrowed
    fn from_bytes(bytes: &[u8]) -> Self;

    /// View `bytes` as the borrowed key form, with no allocation.
    ///
    /// `as_borrowed(k.bytes())` yields a value that compares equal to `k`. Only
    /// ever called with bytes produced by a `bytes()` call of the same key
    /// type, so impls may skip validation (e.g. the `String` impl assumes valid
    /// UTF-8).
    fn as_borrowed<'a>(bytes: &'a [u8]) -> Self::Borrowed<'a>;
}

impl ByteKey for Vec<u8> {
    type Borrowed<'a> = &'a [u8] where Self: 'a;
    fn bytes(&self) -> &[u8] {
        self
    }
    fn from_bytes(bytes: &[u8]) -> Self {
        bytes.to_vec()
    }
    fn as_borrowed<'a>(bytes: &'a [u8]) -> &'a [u8] {
        bytes
    }
}

impl ByteKey for String {
    type Borrowed<'a> = &'a str where Self: 'a;
    fn bytes(&self) -> &[u8] {
        self.as_bytes()
    }
    fn from_bytes(bytes: &[u8]) -> Self {
        // Safe: bytes were originally produced by String::as_bytes (valid UTF-8).
        String::from_utf8(bytes.to_vec()).unwrap()
    }
    fn as_borrowed<'a>(bytes: &'a [u8]) -> &'a str {
        // SAFETY: `as_borrowed` is only called with bytes that originated from a
        // `String::bytes()` call (the trie stores only such bytes), so they are
        // valid UTF-8. Skipping the validation here is the whole point — it
        // avoids re-validating every key on every iteration.
        unsafe { std::str::from_utf8_unchecked(bytes) }
    }
}

impl ByteKey for NonZeroBytes {
    type Borrowed<'a> = &'a [u8] where Self: 'a;
    fn bytes(&self) -> &[u8] {
        self.as_ref()
    }
    /// Panics if bytes contains a 0
    fn from_bytes(bytes: &[u8]) -> Self {
        Self::new(bytes.to_vec()).unwrap()
    }
    fn as_borrowed<'a>(bytes: &'a [u8]) -> &'a [u8] {
        // SAFETY: only called with bytes from a NonZeroBytes key (no 0x00), so
        // the no-zero invariant is preserved by construction; the borrowed
        // view is just the raw slice.
        bytes
    }
}

// ---------------------------------------------------------------------------
// TrieKey implementations
// ---------------------------------------------------------------------------

impl TrieKey for Vec<u8> {
    type Store = BufKeyStore;
    fn as_bytes(&self) -> &[u8] {
        self
    }
}

impl TrieKey for String {
    type Store = VecKeyStore<String>;
    fn as_bytes(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl TrieKey for NonZeroBytes {
    type Store = VecKeyStore<NonZeroBytes>;
    fn as_bytes(&self) -> &[u8] {
        self.as_ref()
    }
}
// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buf_store_push_and_key_bytes() {
        let mut store = BufKeyStore::default();
        assert_eq!(store.len(), 0);

        let ki1 = store.push(b"hello".to_vec());
        assert_eq!(ki1, 1);
        assert_eq!(store.key_bytes(1), b"hello");
        assert_eq!(store.len(), 1);

        let ki2 = store.push(b"world".to_vec());
        assert_eq!(ki2, 2);
        assert_eq!(store.key_bytes(2), b"world");
        assert_eq!(store.len(), 2);
    }

    #[test]
    fn buf_store_rollback() {
        let mut store = BufKeyStore::default();
        store.push(b"hello".to_vec());
        store.push(b"world".to_vec());
        assert_eq!(store.len(), 2);

        store.rollback();
        assert_eq!(store.len(), 1);
        assert_eq!(store.key_bytes(1), b"hello");
    }

    #[test]
    fn buf_store_into_keys() {
        let mut store = BufKeyStore::default();
        store.push(b"abc".to_vec());
        store.push(b"def".to_vec());
        let keys = store.into_keys();
        assert_eq!(keys, vec![b"abc".to_vec(), b"def".to_vec()]);
    }

    #[test]
    fn buf_store_empty_key() {
        let mut store = BufKeyStore::default();
        let ki = store.push(b"".to_vec());
        assert_eq!(ki, 1);
        assert_eq!(store.key_bytes(1), b"");
    }

    #[test]
    fn buf_store_dummy_entry() {
        let store = BufKeyStore::default();
        assert_eq!(store.key_bytes(0), b"");
    }

    #[test]
    fn vec_store_push_and_key_bytes() {
        let mut store = VecKeyStore::<String>::default();
        assert_eq!(store.len(), 0);

        let ki1 = store.push("hello".to_string());
        assert_eq!(ki1, 1);
        assert_eq!(store.key_bytes(1), b"hello");
        assert_eq!(store.len(), 1);

        let ki2 = store.push("world".to_string());
        assert_eq!(ki2, 2);
        assert_eq!(store.key_bytes(2), b"world");
    }

    #[test]
    fn vec_store_rollback() {
        let mut store = VecKeyStore::<String>::default();
        store.push("hello".to_string());
        store.push("world".to_string());
        store.rollback();
        assert_eq!(store.len(), 1);
        assert_eq!(store.key_bytes(1), b"hello");
    }

    #[test]
    fn vec_store_into_keys() {
        let mut store = VecKeyStore::<String>::default();
        store.push("abc".to_string());
        store.push("def".to_string());
        let keys = store.into_keys();
        assert_eq!(keys, vec!["abc".to_string(), "def".to_string()]);
    }

    #[test]
    fn vec_store_dummy_entry() {
        let store = VecKeyStore::<String>::default();
        assert_eq!(store.key_bytes(0), b"");
    }
}
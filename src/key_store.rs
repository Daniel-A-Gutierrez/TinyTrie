//! Key storage traits and implementations for generic trie key types.
//!
//! [`TrieKey`] defines how a key type provides its byte representation for trie
//! traversal, and which [`KeyStore`] implementation backs it. Two store backends
//! are provided:
//!
//! - [`BufKeyStore`] — flat buffer storage for `Vec<u8>` keys (cache-friendly,
//!   single contiguous allocation for all key bytes).
//! - [`VecKeyStore`] — `Vec<K>` storage for any `TrieKey` type (e.g. `String`).

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

// ---------------------------------------------------------------------------
// U64Key — wrapper for u64 that provides big-endian byte representation
// ---------------------------------------------------------------------------

/// A `u64` key wrapper whose byte representation is big-endian.
///
/// Only 8 bytes — the BE byte array is the sole storage; the native u64 value
/// is derived from it via `u64::from_be_bytes`. This gives correct MSB-first
/// bit ordering in the trie (bit 0 = MSB of the big-endian representation)
/// without any redundant storage.
#[derive(Clone, Copy)]
pub struct U64Key([u8; 8]);

impl U64Key {
    pub fn new(value: u64) -> Self {
        U64Key(value.to_be_bytes())
    }

    pub fn value(&self) -> u64 {
        u64::from_be_bytes(self.0)
    }
}

impl Default for U64Key {
    fn default() -> Self {
        U64Key::new(0)
    }
}

impl From<u64> for U64Key {
    fn from(value: u64) -> Self {
        U64Key::new(value)
    }
}

impl std::fmt::Debug for U64Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("U64Key").field(&self.value()).finish()
    }
}

impl TrieKey for U64Key {
    type Store = VecKeyStore<U64Key>;
    fn as_bytes(&self) -> &[u8] {
        &self.0
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
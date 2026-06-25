//! Dynamic NibbleTrie — starts with compact u8 arena indices and promotes
//! to wider types (u16 → u32 → u64) as the trie grows.
//!
//! `DynTrie<T>` wraps an enum over concrete `NibbleTrie<Vec<u8>, T, PTR, u16>`
//! variants. On insert, it checks whether the current PTR type is approaching
//! capacity and promotes automatically. This gives small tries the memory
//! efficiency of u8 indices (32-byte nodes) while supporting unbounded growth.
//!
//! # Dispatch
//!
//! All method calls dispatch through a `match` on the enum variant — no vtable,
//! no `Box`, no heap allocation on promotion. The compiler can inline concrete
//! `NibbleTrie` methods per variant.

use crate::nibble_trie::NibbleTrie;

// ---------------------------------------------------------------------------
// DynInner enum — replaces Box<dyn DynTrie<T>>
// ---------------------------------------------------------------------------

/// Internal enum holding one of the four concrete `NibbleTrie` variants.
/// Promotion replaces the variant in-place via `std::mem::replace`.
enum DynInner<T> {
    U8 (NibbleTrie<Vec<u8>, T, u8,  u16, 2>),
    U16(NibbleTrie<Vec<u8>, T, u16, u16, 2>),
    U32(NibbleTrie<Vec<u8>, T, u32, u16, 2>),
    U64(NibbleTrie<Vec<u8>, T, u64, u16, 2>),
}

// ---------------------------------------------------------------------------
// DynTrie
// ---------------------------------------------------------------------------

/// A nibble trie that starts with compact u8 arena indices and automatically
/// promotes to u16/u32/u64 as needed.
///
/// Node sizes by PTR width:
/// - u8:  32 bytes (fits half a cache line)
/// - u16: 48 bytes (fits a cache line)
/// - u32: 80 bytes
/// - u64: 152 bytes
///
/// Promotion is transparent: insert checks capacity and promotes before the
/// underlying trie overflows. No vtable dispatch or heap allocation on
/// promotion — just an enum variant swap.
pub struct DynTrie<T> {
    inner: DynInner<T>,
}

impl<T> DynTrie<T> {
    /// Create an empty DynTrie starting with u8 arena indices (32-byte nodes).
    pub fn new() -> Self {
        DynTrie {
            inner: DynInner::U8(NibbleTrie::new()),
        }
    }

    /// Look up a key. Returns the key index if found.
    pub fn get(&self, key: &[u8]) -> Option<usize> {
        match &self.inner {
            DynInner::U8(t) => t.get(key),
            DynInner::U16(t) => t.get(key),
            DynInner::U32(t) => t.get(key),
            DynInner::U64(t) => t.get(key),
        }
    }

    /// Insert a key-value pair. Automatically promotes to a wider PTR type if
    /// the current one is approaching capacity. Returns the key index on
    /// success, or `Err(())` on duplicate key.
    pub fn insert(&mut self, key: Vec<u8>, value: T) -> Result<usize, ()> {
        // Promote while at capacity (handles chained promotion, e.g. u8→u16→u32
        // if the capacity threshold is overshot).
        loop {
            let near = match &self.inner {
                DynInner::U8(t) => t.near_capacity(),
                DynInner::U16(t) => t.near_capacity(),
                DynInner::U32(t) => t.near_capacity(),
                DynInner::U64(t) => t.near_capacity(),
            };
            if !near {
                break;
            }
            self.promote_inner();
        }
        match &mut self.inner {
            DynInner::U8(t) => t.insert(key, value),
            DynInner::U16(t) => t.insert(key, value),
            DynInner::U32(t) => t.insert(key, value),
            DynInner::U64(t) => t.insert(key, value),
        }
    }

    /// Number of entries in the trie.
    pub fn len(&self) -> usize {
        match &self.inner {
            DynInner::U8(t) => t.len(),
            DynInner::U16(t) => t.len(),
            DynInner::U32(t) => t.len(),
            DynInner::U64(t) => t.len(),
        }
    }

    /// Returns `true` if the trie is empty.
    pub fn is_empty(&self) -> bool {
        match &self.inner {
            DynInner::U8(t) => t.is_empty(),
            DynInner::U16(t) => t.is_empty(),
            DynInner::U32(t) => t.is_empty(),
            DynInner::U64(t) => t.is_empty(),
        }
    }

    /// Optimize the trie's memory layout for cache locality.
    pub fn optimize(&mut self) {
        match &mut self.inner {
            DynInner::U8(t) => t.optimize(),
            DynInner::U16(t) => t.optimize(),
            DynInner::U32(t) => t.optimize(),
            DynInner::U64(t) => t.optimize(),
        }
    }

    /// Returns the size of the current PTR type in bytes (1, 2, 4, or 8).
    /// Useful for debugging and testing promotion.
    pub fn ptr_size(&self) -> usize {
        match &self.inner {
            DynInner::U8(_) => 1,
            DynInner::U16(_) => 2,
            DynInner::U32(_) => 4,
            DynInner::U64(_) => 8,
        }
    }

    /// Iterate all key-value pairs in forward (ascending) order.
    pub fn iter_fwd(&self, f: &mut dyn FnMut(&[u8], &T)) {
        match &self.inner {
            DynInner::U8(t) => {
                let mut it = t.iter();
                if let Some((k, v)) = it.current() { f(k, v); }
                while let Some((k, v)) = it.next() { f(k, v); }
            }
            DynInner::U16(t) => {
                let mut it = t.iter();
                if let Some((k, v)) = it.current() { f(k, v); }
                while let Some((k, v)) = it.next() { f(k, v); }
            }
            DynInner::U32(t) => {
                let mut it = t.iter();
                if let Some((k, v)) = it.current() { f(k, v); }
                while let Some((k, v)) = it.next() { f(k, v); }
            }
            DynInner::U64(t) => {
                let mut it = t.iter();
                if let Some((k, v)) = it.current() { f(k, v); }
                while let Some((k, v)) = it.next() { f(k, v); }
            }
        }
    }

    /// Iterate all key-value pairs in reverse (descending) order.
    pub fn iter_rev(&self, f: &mut dyn FnMut(&[u8], &T)) {
        match &self.inner {
            DynInner::U8(t) => {
                let mut it = t.iter_last();
                if let Some((k, v)) = it.current() { f(k, v); }
                while let Some((k, v)) = it.prev() { f(k, v); }
            }
            DynInner::U16(t) => {
                let mut it = t.iter_last();
                if let Some((k, v)) = it.current() { f(k, v); }
                while let Some((k, v)) = it.prev() { f(k, v); }
            }
            DynInner::U32(t) => {
                let mut it = t.iter_last();
                if let Some((k, v)) = it.current() { f(k, v); }
                while let Some((k, v)) = it.prev() { f(k, v); }
            }
            DynInner::U64(t) => {
                let mut it = t.iter_last();
                if let Some((k, v)) = it.current() { f(k, v); }
                while let Some((k, v)) = it.prev() { f(k, v); }
            }
        }
    }

    /// Manually demote to a narrower PTR type if the trie is small enough.
    /// Returns `Ok(())` on success, `Err(())` if the trie is too large or
    /// already at the minimum width (u8).
    pub fn demote(&mut self) -> Result<(), ()> {
        // Take ownership of inner via replace, then call consuming demote()
        let inner = std::mem::replace(&mut self.inner, DynInner::U8(NibbleTrie::new()));
        match inner {
            DynInner::U8(t) => {
                // u8 is the minimum — can't demote further
                self.inner = DynInner::U8(t);
                Err(())
            }
            DynInner::U16(t) => {
                match t.demote::<u8>() {
                    Ok(smaller) => {
                        self.inner = DynInner::U8(smaller);
                        Ok(())
                    }
                    Err(original) => {
                        self.inner = DynInner::U16(original);
                        Err(())
                    }
                }
            }
            DynInner::U32(t) => {
                match t.demote::<u16>() {
                    Ok(smaller) => {
                        self.inner = DynInner::U16(smaller);
                        Ok(())
                    }
                    Err(original) => {
                        self.inner = DynInner::U32(original);
                        Err(())
                    }
                }
            }
            DynInner::U64(t) => {
                match t.demote::<u32>() {
                    Ok(smaller) => {
                        self.inner = DynInner::U32(smaller);
                        Ok(())
                    }
                    Err(original) => {
                        self.inner = DynInner::U64(original);
                        Err(())
                    }
                }
            }
        }
    }

    /// Promote the inner trie to the next wider PTR type.
    fn promote_inner(&mut self) {
        // We need to take ownership of the inner trie, promote it, and replace.
        // Use a placeholder that will be immediately overwritten.
        let inner = std::mem::replace(&mut self.inner, DynInner::U8(NibbleTrie::new()));
        self.inner = match inner {
            DynInner::U8(t) => DynInner::U16(t.promote()),
            DynInner::U16(t) => DynInner::U32(t.promote()),
            DynInner::U32(t) => DynInner::U64(t.promote()),
            DynInner::U64(t) => {
                // u64 is the maximum — this should never happen
                // (near_capacity returns false for u64 in practice)
                DynInner::U64(t)
            }
        };
    }
}

impl<T> Default for DynTrie<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "tests/dyn_trie.rs"]
mod tests;

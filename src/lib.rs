#![feature(iter_array_chunks)]
#![feature(generic_const_exprs)]
#![feature(portable_simd)]

//! Compact DFA String Index
//!
//! A prefix-compressed radix trie with existence guarantee, viewed as a
//! deterministic finite automaton. Node size is determined by const generics:
//! - `INLINE`: max inline children count (tag 2..=INLINE)
//! - `PREFIX`: prefix length type (u8, u16, or u32)
//!
//! # Safety: Copy union with raw pointers
//!
//! `Trie` is a tagged union whose variants contain raw pointers (`*mut Trie`,
//! `*mut u8`). All variants (`INode`, `PairVec`, `Leaf`) derive `Copy` so that
//! `Trie` itself is `Copy` — this allows direct union field access without
//! `ManuallyDrop` wrappers, which reduces overhead in the hot path.
//!
//! Copy is safe here because ownership is managed explicitly, not implicitly:
//! - `TinyTrie` owns the root `Box<Trie>` and recursively frees subtrees in
//!   its `Drop` impl. Implicit copies of `Trie` values (e.g., via `ptr::read`)
//!   are bitwise copies of the struct; they do not claim ownership of the
//!   pointed-to heap allocations.
//! - `InternalNodeOwned` holds an owned inode/pairvec extracted via `ptr::read`.
//! - `Trie` has no `Drop` impl, so copying a `Trie` value does not double-free.
//!
//! # Null-Terminator Contract
//!
//! All keys stored in the trie are null-terminated internally (a `0x00` byte is
//! appended). This means:
//!
//! - `insert()` rejects keys containing `0x00` bytes and appends the terminator
//!   internally before storing.
//! - `get()` and `seek()` **require** null-terminated input. Callers can use
//!   [`null_terminate`] to add the terminator, or pass a null-terminated `&[u8]`
//!   directly (e.g., `b"hello\0"`).
//! - `TrieIter::current()` returns keys **without** the null terminator,
//!   matching the `insert` API.
//!
//! The null byte serves as an implicit sentinel: a leaf node's key always ends
//! with `0x00`, which acts as a unique terminator distinguishing "ab" from
//! "abc" during prefix comparison.

mod simd;

mod bit_trie;
pub use bit_trie::BitTrie;

mod nibble_trie;
pub use nibble_trie::{NibbleTrie, Node, TrieIndex};

#[cfg(feature = "archive")]
pub use archive::prefix_trie::null_terminate;

mod dyn_nibble_trie;
pub use dyn_nibble_trie::DynNibbleTrie;

mod tiny_trie_map;
pub use tiny_trie_map::TinyTrieMap;

#[cfg(feature = "archive")]
mod archive;

// ---------------------------------------------------------------------------
// Corpus key loading (shared between trie-stats and bench)
// ---------------------------------------------------------------------------

/// Load unique sorted keys from a file, one per line.
pub fn load_corpus_lines(path: &str) -> Vec<Vec<u8>> {
    let data = std::fs::read(path).unwrap_or_else(|e| {
        eprintln!("Failed to read corpus '{}': {e}", path);
        std::process::exit(1);
    });
    let mut keys: Vec<Vec<u8>> = data.split(|&b| b == b'\n')
        .map(|line| {
            let mut v = line.to_vec();
            v.truncate(v.len().saturating_sub(b"\r".len()));
            v
        })
        .filter(|line| !line.is_empty())
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

/// Load unique sorted keys from a file, split by whitespace.
pub fn load_corpus_words(path: &str) -> Vec<Vec<u8>> {
    let data = std::fs::read(path).unwrap_or_else(|e| {
        eprintln!("Failed to read corpus '{}': {e}", path);
        std::process::exit(1);
    });
    let mut keys: Vec<Vec<u8>> = data.split(|&b| b.is_ascii_whitespace())
        .map(|w| w.to_vec())
        .filter(|w| !w.is_empty())
        .collect();
    keys.sort();
    keys.dedup();
    keys
}
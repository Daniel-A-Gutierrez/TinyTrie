use super::*;
use std::mem::size_of;

type FLN<T> = FixedLenNibbleTrie<T, u32>;
type FLN16<T> = FixedLenNibbleTrie<T, u16>;

// ---------------------------------------------------------------------------
// Node sizes
// ---------------------------------------------------------------------------

#[test]
fn node_size_u16() {
    assert_eq!(size_of::<FixedLenNode<u16>>(), 40, "FixedLenNode<u16> should be 40 bytes");
}

#[test]
fn node_size_u32() {
    assert_eq!(size_of::<FixedLenNode<u32>>(), 76, "FixedLenNode<u32> should be 76 bytes");
}

#[test]
fn node_size_u64() {
    // u64: [u64; 16] = 128 bytes + u16(2) + u16(2) + u64(8) + u8(1) + pad(7) = 148
    // Actual layout may differ — just check it's reasonable (less than NibbleTrie's u64 node)
    assert!(size_of::<FixedLenNode<u64>>() < 152);
}

// ---------------------------------------------------------------------------
// Basic insert/get
// ---------------------------------------------------------------------------

#[test]
fn insert_empty_key() {
    let mut trie: FLN<usize> = FixedLenNibbleTrie::new(16);
    let idx = trie.insert(vec![], 42).unwrap();
    assert_eq!(idx, 0);
    assert_eq!(trie.get(b""), Some(&42));
}

#[test]
fn insert_single_key() {
    let mut trie: FLN<usize> = FixedLenNibbleTrie::new(16);
    let idx = trie.insert(b"hello".to_vec(), 1).unwrap();
    assert_eq!(idx, 0);
    assert_eq!(trie.get(b"hello"), Some(&1));
}

#[test]
fn insert_two_keys() {
    let mut trie: FLN<usize> = FixedLenNibbleTrie::new(16);
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"abd".to_vec(), 2).unwrap();
    assert_eq!(trie.get(b"abc"), Some(&1));
    assert_eq!(trie.get(b"abd"), Some(&2));
    assert_eq!(trie.get(b"ab"), None);
    assert_eq!(trie.get(b"abcd"), None);
}

#[test]
fn insert_duplicate_returns_error() {
    let mut trie: FLN<usize> = FixedLenNibbleTrie::new(16);
    trie.insert(b"abc".to_vec(), 1).unwrap();
    assert!(trie.insert(b"abc".to_vec(), 2).is_err());
    assert_eq!(trie.len(), 1);
}

#[test]
fn insert_no_common_prefix() {
    let mut trie: FLN<usize> = FixedLenNibbleTrie::new(16);
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"xyz".to_vec(), 2).unwrap();
    assert_eq!(trie.get(b"abc"), Some(&1));
    assert_eq!(trie.get(b"xyz"), Some(&2));
}

#[test]
fn insert_prefix_key() {
    let mut trie: FLN<usize> = FixedLenNibbleTrie::new(16);
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"abcd".to_vec(), 2).unwrap();
    assert_eq!(trie.get(b"abc"), Some(&1));
    assert_eq!(trie.get(b"abcd"), Some(&2));
}

#[test]
fn insert_reverse_prefix_key() {
    let mut trie: FLN<usize> = FixedLenNibbleTrie::new(16);
    trie.insert(b"abcd".to_vec(), 1).unwrap();
    trie.insert(b"abc".to_vec(), 2).unwrap();
    assert_eq!(trie.get(b"abcd"), Some(&1));
    assert_eq!(trie.get(b"abc"), Some(&2));
}

#[test]
fn insert_three_keys() {
    let mut trie: FLN<usize> = FixedLenNibbleTrie::new(16);
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"abd".to_vec(), 2).unwrap();
    trie.insert(b"abe".to_vec(), 3).unwrap();
    assert_eq!(trie.get(b"abc"), Some(&1));
    assert_eq!(trie.get(b"abd"), Some(&2));
    assert_eq!(trie.get(b"abe"), Some(&3));
}

#[test]
fn insert_many_keys_same_prefix() {
    let mut trie: FLN<usize> = FixedLenNibbleTrie::new(16);
    for i in 0..50 {
        let key = format!("prefix_{:02}", i);
        trie.insert(key.into_bytes(), i).unwrap();
    }
    assert_eq!(trie.len(), 50);
    for i in 0..50 {
        let key = format!("prefix_{:02}", i);
        assert_eq!(trie.get(&key.into_bytes()), Some(&i));
    }
}

// ---------------------------------------------------------------------------
// Key constraints
// ---------------------------------------------------------------------------

#[test]
fn key_with_embedded_null() {
    // Keys with embedded null bytes (not trailing) should work
    let mut trie: FLN<usize> = FixedLenNibbleTrie::new(16);
    trie.insert(b"a\x00b".to_vec(), 1).unwrap();
    assert_eq!(trie.get(b"a\x00b"), Some(&1));
}

#[test]
fn key_max_length() {
    let mut trie: FLN<usize> = FixedLenNibbleTrie::new(8);
    let key = vec![b'x'; 8];
    trie.insert(key.clone(), 1).unwrap();
    assert_eq!(trie.get(&key), Some(&1));
}

#[test]
fn key_too_long_rejected() {
    let mut trie: FLN<usize> = FixedLenNibbleTrie::new(8);
    let key = vec![b'x'; 9];
    assert!(trie.insert(key, 1).is_err());
}

#[test]
fn trailing_zero_key_preserved() {
    // With lens storing the true length, keys with trailing zeros are preserved
    let mut trie: FLN<usize> = FixedLenNibbleTrie::new(16);
    trie.insert(b"a\x00".to_vec(), 1).unwrap();
    // "a\0" has length 2, so get(b"a") should NOT match (different length)
    assert_eq!(trie.get(b"a"), None);
    // get(b"a\0") should match
    assert_eq!(trie.get(b"a\x00"), Some(&1));
}

#[test]
fn len_and_is_empty() {
    let mut trie: FLN<usize> = FixedLenNibbleTrie::new(16);
    assert!(trie.is_empty());
    assert_eq!(trie.len(), 0);
    trie.insert(b"abc".to_vec(), 1).unwrap();
    assert!(!trie.is_empty());
    assert_eq!(trie.len(), 1);
    trie.insert(b"abd".to_vec(), 2).unwrap();
    assert_eq!(trie.len(), 2);
}

// ---------------------------------------------------------------------------
// Compact mode (u16 PTR)
// ---------------------------------------------------------------------------

#[test]
fn compact_u16_ptr() {
    let mut trie: FLN16<usize> = FixedLenNibbleTrie::new(16);
    for i in 0..100u8 {
        let key = format!("key_{:03}", i);
        trie.insert(key.into_bytes(), i as usize).unwrap();
    }
    assert_eq!(trie.len(), 100);
    for i in 0..100u8 {
        let key = format!("key_{:03}", i);
        let v = i as usize;
        assert_eq!(trie.get(&key.into_bytes()), Some(&v));
    }
}

// ---------------------------------------------------------------------------
// Iteration
// ---------------------------------------------------------------------------

#[test]
fn iter_empty() {
    let trie: FLN<usize> = FixedLenNibbleTrie::new(16);
    let mut it = trie.iter();
    assert!(it.current().is_none());
    assert!(it.next().is_none());
}

#[test]
fn iter_single_key() {
    let mut trie: FLN<usize> = FixedLenNibbleTrie::new(16);
    trie.insert(b"abc".to_vec(), 1).unwrap();
    let mut it = trie.iter();
    // Root is not terminal, so current() returns None; next() gives first key
    let (k, v) = it.next().unwrap();
    assert_eq!(k, b"abc");
    assert_eq!(*v, 1);
    assert!(it.next().is_none());
}

#[test]
fn iter_forward() {
    let mut trie: FLN<usize> = FixedLenNibbleTrie::new(16);
    let keys = vec![b"abc".to_vec(), b"abd".to_vec(), b"abe".to_vec(), b"xyz".to_vec()];
    for (i, k) in keys.iter().enumerate() {
        trie.insert(k.clone(), i).unwrap();
    }
    let mut collected = Vec::new();
    let mut it = trie.iter();
    if let Some((k, v)) = it.current() {
        collected.push((k.to_vec(), *v));
    }
    while let Some((k, v)) = it.next() {
        collected.push((k.to_vec(), *v));
    }
    // Should be in sorted order
    let mut expected: Vec<_> = keys.into_iter().enumerate().collect::<Vec<_>>();
    expected.sort_by(|a, b| a.1.cmp(&b.1));
    for (i, (_, expected_k)) in expected.iter().enumerate() {
        assert_eq!(&collected[i].0, expected_k, "sorted key {} mismatch", i);
    }
    assert_eq!(collected.len(), 4);
}

#[test]
fn iter_backward() {
    let mut trie: FLN<usize> = FixedLenNibbleTrie::new(16);
    let keys = vec![b"abc".to_vec(), b"abd".to_vec(), b"abe".to_vec(), b"xyz".to_vec()];
    for (i, k) in keys.iter().enumerate() {
        trie.insert(k.clone(), i).unwrap();
    }
    let mut collected = Vec::new();
    let mut it = trie.iter_last();
    if let Some((k, v)) = it.current() {
        collected.push((k.to_vec(), *v));
    }
    while let Some((k, v)) = it.prev() {
        collected.push((k.to_vec(), *v));
    }
    // Should be in reverse sorted order
    let mut sorted_keys: Vec<_> = keys.clone();
    sorted_keys.sort();
    sorted_keys.reverse();
    for (i, expected_k) in sorted_keys.iter().enumerate() {
        assert_eq!(&collected[i].0, expected_k, "reverse key {} mismatch", i);
    }
    assert_eq!(collected.len(), 4);
}

#[test]
fn iter_seek() {
    let mut trie: FLN<usize> = FixedLenNibbleTrie::new(16);
    for i in 0u8..10 {
        let key = format!("key_{:02}", i);
        trie.insert(key.into_bytes(), i as usize).unwrap();
    }
    let mut it = trie.iter();
    let (k, _) = it.seek(b"key_05").unwrap();
    assert_eq!(k, b"key_05");
}

#[test]
fn iter_index() {
    let mut trie: FLN<usize> = FixedLenNibbleTrie::new(16);
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"abd".to_vec(), 2).unwrap();
    trie.insert(b"xyz".to_vec(), 3).unwrap();

    let mut indices = Vec::new();
    let mut it = trie.iter();
    if let Some(i) = it.current_index() { indices.push(i); }
    while let Some(i) = it.next_index() { indices.push(i); }
    assert_eq!(indices.len(), 3);
}

// ---------------------------------------------------------------------------
// Optimize
// ---------------------------------------------------------------------------

#[test]
fn optimize_preserves_lookups() {
    let mut trie: FLN<usize> = FixedLenNibbleTrie::new(16);
    let keys: Vec<Vec<u8>> = (0..100).map(|i| format!("key_{:03}", i).into_bytes()).collect();
    for (i, k) in keys.iter().enumerate() {
        trie.insert(k.clone(), i).unwrap();
    }
    trie.optimize();
    for (i, k) in keys.iter().enumerate() {
        assert_eq!(trie.get(k), Some(&i), "key {:?} not found after optimize", String::from_utf8_lossy(k));
    }
}

#[test]
fn optimize_preserves_iteration() {
    let mut trie: FLN<usize> = FixedLenNibbleTrie::new(16);
    let keys: Vec<Vec<u8>> = (0..50).map(|i| format!("key_{:03}", i).into_bytes()).collect();
    for (i, k) in keys.iter().enumerate() {
        trie.insert(k.clone(), i).unwrap();
    }
    trie.optimize();

    let mut collected = Vec::new();
    let mut it = trie.iter();
    if let Some((k, v)) = it.current() { collected.push((k.to_vec(), *v)); }
    while let Some((k, v)) = it.next() { collected.push((k.to_vec(), *v)); }

    let mut sorted_keys: Vec<_> = keys.clone();
    sorted_keys.sort();
    for (i, expected_k) in sorted_keys.iter().enumerate() {
        assert_eq!(&collected[i].0, expected_k, "sorted key {} mismatch", i);
    }
}

#[test]
fn optimize_sorts_buf() {
    let mut trie: FLN<usize> = FixedLenNibbleTrie::new(16);
    // Insert in reverse order so buf is not sorted
    for i in (0..20u8).rev() {
        let key = format!("key_{:02}", i);
        trie.insert(key.into_bytes(), i as usize).unwrap();
    }
    trie.optimize();

    // After optimize, keys should be laid out contiguously in sorted order in buf
    // Verify by iterating and checking index order
    let mut it = trie.iter();
    let mut prev_idx: Option<usize> = None;
    while let Some(idx) = it.next_index() {
        if let Some(pi) = prev_idx {
            assert!(idx > pi, "indices not in ascending order: {} vs {}", pi, idx);
        }
        prev_idx = Some(idx);
    }
}

#[test]
fn optimize_idempotent() {
    let mut trie: FLN<usize> = FixedLenNibbleTrie::new(16);
    for i in 0..20 {
        let key = format!("key_{:03}", i);
        trie.insert(key.into_bytes(), i).unwrap();
    }
    trie.optimize();
    let buf_after_first = trie.buf.clone();
    let values_after_first: Vec<usize> = trie.values.clone();

    trie.optimize();
    // Second optimize should not change anything
    assert_eq!(trie.buf, buf_after_first);
    assert_eq!(trie.values, values_after_first);
}

// ---------------------------------------------------------------------------
// Auto-optimize at power of two
// ---------------------------------------------------------------------------

#[test]
fn auto_optimize_at_power_of_two() {
    let mut trie: FLN<usize> = FixedLenNibbleTrie::new(16);
    // Insert 8 keys with auto-optimize
    for i in 0..8 {
        let key = format!("key_{:02}", i);
        trie.insert_auto(key.into_bytes(), i).unwrap();
    }
    // Should still find all keys
    for i in 0..8 {
        let key = format!("key_{:02}", i);
        assert_eq!(trie.get(&key.into_bytes()), Some(&i));
    }
}

// ---------------------------------------------------------------------------
// into_keys_values
// ---------------------------------------------------------------------------

#[test]
fn into_keys_values() {
    let mut trie: FLN<usize> = FixedLenNibbleTrie::new(16);
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"xyz".to_vec(), 2).unwrap();
    let (keys, values) = trie.into_keys_values();
    assert_eq!(keys.len(), 2);
    assert_eq!(values.len(), 2);
}

// ---------------------------------------------------------------------------
// Empty key
// ---------------------------------------------------------------------------

#[test]
fn empty_key_as_terminal() {
    let mut trie: FLN<usize> = FixedLenNibbleTrie::new(16);
    trie.insert(vec![], 99).unwrap();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    assert_eq!(trie.get(b""), Some(&99));
    assert_eq!(trie.get(b"abc"), Some(&1));
}

// ---------------------------------------------------------------------------
// Many keys stress test
// ---------------------------------------------------------------------------

#[test]
fn many_keys() {
    let mut trie: FLN<usize> = FixedLenNibbleTrie::new(16);
    let n = 1000;
    for i in 0..n {
        let key = format!("key_{:06}", i);
        trie.insert(key.into_bytes(), i).unwrap();
    }
    assert_eq!(trie.len(), n);
    for i in 0..n {
        let key = format!("key_{:06}", i);
        assert_eq!(trie.get(&key.into_bytes()), Some(&i), "missing key {}", i);
    }
    // Test iteration count
    let mut count = 0;
    let mut it = trie.iter();
    while it.next().is_some() { count += 1; }
    if it.current().is_some() { count += 1; }
    assert_eq!(count, n);
}
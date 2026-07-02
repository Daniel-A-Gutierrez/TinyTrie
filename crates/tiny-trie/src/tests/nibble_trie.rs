use super::*;

#[test]
fn node_size_default() {
    // Default PTR=u32, LEN=u16: 64 children + 2 prefix_len + 2 leaf_mask
    // + 4 leaf + 1 terminal + 3 padding = 76 bytes
    assert_eq!(std::mem::size_of::<Node<u32, u16>>(), 76);
}

#[test]
fn node_size_compact() {
    // Compact PTR=u16, LEN=u16: 32 children + 2 prefix_len + 2 leaf_mask
    // + 2 leaf + 1 terminal + 1 padding = 40 bytes
    assert_eq!(std::mem::size_of::<Node<u16, u16>>(), 40);
}

#[test]
fn insert_empty_and_get() {
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    let idx = trie.insert(b"hello".to_vec(), 42).unwrap();
    assert_eq!(trie.get(b"hello"), Some(idx));
    assert_eq!(trie.get_value(b"hello"), Some(&42));
    assert_eq!(trie.get(b"world"), None);
}

#[test]
fn insert_duplicate_returns_error() {
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    trie.insert(b"hello".to_vec(), 1).unwrap();
    let result = trie.insert(b"hello".to_vec(), 2);
    assert_eq!(result, Err(()));
    assert_eq!(trie.len(), 1);
}

#[test]
fn insert_null_byte_allowed() {
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    // Null bytes are now valid in keys
    let idx = trie.insert(b"hel\0lo".to_vec(), 1).unwrap();
    assert_eq!(trie.get(b"hel\0lo"), Some(idx));
}

#[test]
fn insert_two_keys_split_leaf() {
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    let i1 = trie.insert(b"abc".to_vec(), 1).unwrap();
    let i2 = trie.insert(b"abd".to_vec(), 2).unwrap();
    assert_eq!(trie.get(b"abc"), Some(i1));
    assert_eq!(trie.get(b"abd"), Some(i2));
    assert_eq!(trie.len(), 2);
}

#[test]
fn insert_prefix_key() {
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    let i1 = trie.insert(b"abc".to_vec(), 1).unwrap();
    let i2 = trie.insert(b"abcd".to_vec(), 2).unwrap();
    assert_eq!(trie.get(b"abc"), Some(i1));
    assert_eq!(trie.get(b"abcd"), Some(i2));
}

#[test]
fn insert_reverse_prefix_key() {
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    let i1 = trie.insert(b"abcd".to_vec(), 1).unwrap();
    let i2 = trie.insert(b"abc".to_vec(), 2).unwrap();
    assert_eq!(trie.get(b"abcd"), Some(i1));
    assert_eq!(trie.get(b"abc"), Some(i2));
}

#[test]
fn insert_no_common_prefix() {
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    let i1 = trie.insert(b"abc".to_vec(), 1).unwrap();
    let i2 = trie.insert(b"xyz".to_vec(), 2).unwrap();
    assert_eq!(trie.get(b"abc"), Some(i1));
    assert_eq!(trie.get(b"xyz"), Some(i2));
}

#[test]
fn insert_three_keys() {
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    let i1 = trie.insert(b"abc".to_vec(), 1).unwrap();
    let i2 = trie.insert(b"abd".to_vec(), 2).unwrap();
    let i3 = trie.insert(b"abe".to_vec(), 3).unwrap();
    assert_eq!(trie.get(b"abc"), Some(i1));
    assert_eq!(trie.get(b"abd"), Some(i2));
    assert_eq!(trie.get(b"abe"), Some(i3));
}

#[test]
fn insert_single_char_keys() {
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    let mut indices = Vec::new();
    for c in b'a'..=b'f' {
        let idx = trie.insert(vec![c], c as i32).unwrap();
        indices.push(idx);
    }
    for (i, c) in (b'a'..=b'f').enumerate() {
        let key = vec![c];
        assert_eq!(trie.get(&key), Some(indices[i]));
    }
}

#[test]
fn insert_many_keys_same_prefix() {
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    for i in 0..50 {
        let key = format!("prefix_{:02}", i);
        trie.insert(key.into_bytes(), i).unwrap();
    }
    for i in 0..50 {
        let key = format!("prefix_{:02}", i);
        assert!(trie.get(key.as_bytes()).is_some());
    }
}

#[test]
fn insert_deeply_nested() {
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    let mut key = Vec::new();
    for i in 0..100 {
        key.push(b'a');
        let idx = trie.insert(key.clone(), i).unwrap();
        assert_eq!(trie.get(&key), Some(idx));
    }
}

#[test]
fn len_and_is_empty() {
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    assert!(trie.is_empty());
    assert_eq!(trie.len(), 0);
    trie.insert(b"hello".to_vec(), 1).unwrap();
    assert!(!trie.is_empty());
    assert_eq!(trie.len(), 1);
}

#[test]
fn into_keys_values_roundtrip() {
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"def".to_vec(), 2).unwrap();
    let (keys, values) = trie.into_keys_values();
    assert_eq!(keys, vec![b"abc".to_vec(), b"def".to_vec()]);
    assert_eq!(values, vec![1, 2]);
}

#[test]
fn iter_empty() {
    let trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    let mut iter = trie.iter();
    assert!(iter.next().is_none());
}

#[test]
fn iter_single_key() {
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    trie.insert(b"hello".to_vec(), 42).unwrap();
    let mut iter = trie.iter();
    let (k, v) = iter.next().unwrap();
    assert_eq!(k, b"hello");
    assert_eq!(*v, 42);
    assert!(iter.next().is_none());
}

#[test]
fn iter_forward() {
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"abd".to_vec(), 2).unwrap();
    trie.insert(b"abe".to_vec(), 3).unwrap();

    let mut results = Vec::new();
    let mut iter = trie.iter();
    while let Some((k, _)) = iter.next() {
        results.push(k.to_vec());
    }
    assert_eq!(results, vec![b"abc", b"abd", b"abe"]);
}

#[test]
fn iter_backward() {
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"abd".to_vec(), 2).unwrap();
    trie.insert(b"abe".to_vec(), 3).unwrap();

    let mut iter = trie.iter_last();
    let mut results = Vec::new();
    loop {
        match iter.current() {
            Some((k, _)) => results.push(k.to_vec()),
            None => break,
        }
        if iter.prev().is_none() {
            break;
        }
    }
    assert_eq!(results, vec![b"abe", b"abd", b"abc"]);
}

#[test]
fn iter_seek_exact() {
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"abd".to_vec(), 2).unwrap();
    trie.insert(b"abe".to_vec(), 3).unwrap();

    let mut iter = trie.iter();
    let (k, _) = iter.seek(b"abd").unwrap();
    assert_eq!(k, b"abd");
}

#[test]
fn iter_seek_between() {
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"abd".to_vec(), 2).unwrap();
    trie.insert(b"abe".to_vec(), 3).unwrap();

    // Seek to exact key "abd" works
    let mut iter = trie.iter();
    let (k, _) = iter.seek(b"abd").unwrap();
    assert_eq!(k, b"abd");

    // Seek to key between "abc" and "abd" should return "abd"
    let mut iter2 = trie.iter();
    let result = iter2.seek(b"abc\x7f");
    assert!(result.is_some(), "seek to 'abc\\x7f' returned None");
    assert_eq!(result.unwrap().0, b"abd");
}

#[test]
fn iter_seek_prefix_key() {
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"abcd".to_vec(), 2).unwrap();

    let mut iter = trie.iter();
    let (k, _) = iter.seek(b"abc").unwrap();
    assert_eq!(k, b"abc");
}

#[test]
fn get_value_found_and_missing() {
    let mut trie: NibbleTrie<Vec<u8>, String> = NibbleTrie::new();
    trie.insert(b"hello".to_vec(), "world".to_string()).unwrap();
    assert_eq!(trie.get_value(b"hello"), Some(&"world".to_string()));
    assert_eq!(trie.get_value(b"world"), None);
}

#[test]
fn iter_backward_large() {
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    for i in 0..100 {
        let key = format!("key_{:03}", i);
        trie.insert(key.into_bytes(), i).unwrap();
    }

    let mut iter = trie.iter_last();
    let mut count = 0;
    let mut last_key: Vec<u8> = Vec::new();
    if let Some((k, _)) = iter.current() {
        last_key = k.to_vec();
        count += 1;
    }
    while let Some((k, _)) = iter.prev() {
        assert!(k < &last_key[..], "not descending: {:?} >= {:?}",
            String::from_utf8_lossy(k), String::from_utf8_lossy(&last_key));
        last_key = k.to_vec();
        count += 1;
    }
    assert_eq!(count, 100);
}

#[test]
fn leaf_and_offset_set_on_creation() {
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    // Root should have leaf field set (not the empty sentinel)
    let root = &trie.arena[0];
    assert!(root.leaf.is_some(), "root leaf field should be set");
    // offset should point to the key in buf (stored in index, not node)
    let (off, len) = trie.index[root.leaf.get().as_usize()];
    assert_ne!(off, 0, "root offset should be set");
    assert_eq!(&trie.buf[off..off + len.as_usize()], b"abc");
}

// ── optimize() tests ──────────────────────────────────────────────

#[test]
fn optimize_empty() {
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    trie.optimize();
    assert!(trie.is_empty());
}

#[test]
fn optimize_single_key() {
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    let idx = trie.insert(b"hello".to_vec(), 42).unwrap();
    trie.optimize();
    assert_eq!(trie.get(b"hello"), Some(idx));
    assert_eq!(trie.len(), 1);
}

#[test]
fn optimize_preserves_lookups() {
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    let mut indices = Vec::new();
    for i in 0..100 {
        let key = format!("key_{:03}", i);
        let idx = trie.insert(key.into_bytes(), i).unwrap();
        indices.push(idx);
    }
    trie.optimize();
    for i in 0..100 {
        let key = format!("key_{:03}", i);
        assert_eq!(trie.get(key.as_bytes()), Some(indices[i]),
            "lookup failed after optimize for i={}", i);
    }
    assert_eq!(trie.len(), 100);
}

#[test]
fn optimize_preserves_iteration() {
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    for i in 0..100 {
        let key = format!("key_{:05}", i);
        trie.insert(key.into_bytes(), i as i32).unwrap();
    }
    trie.optimize();

    // Forward
    let mut it = trie.iter();
    let mut keys: Vec<Vec<u8>> = Vec::new();
    while let Some((k, _)) = it.next() {
        keys.push(k.to_vec());
    }
    assert_eq!(keys.len(), 100);
    for i in 1..keys.len() {
        assert!(keys[i] > keys[i - 1], "not sorted after optimize at index {}", i);
    }

    // Backward
    let mut it = trie.iter_last();
    keys.clear();
    loop {
        match it.current() {
            Some((k, _)) => keys.push(k.to_vec()),
            None => break,
        }
        if it.prev().is_none() { break; }
    }
    assert_eq!(keys.len(), 100);
    for i in 1..keys.len() {
        assert!(keys[i] < keys[i - 1], "not reverse-sorted after optimize at index {}", i);
    }
}

#[test]
fn optimize_preserves_seek() {
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    for i in 0..50u32 {
        let key = format!("key_{:05}", i);
        trie.insert(key.into_bytes(), i as i32).unwrap();
    }
    trie.optimize();
    let mut it = trie.iter();
    let (k, v) = it.seek(b"key_00025").unwrap();
    assert_eq!(k, b"key_00025");
    assert_eq!(*v, 25);
}

#[test]
fn optimize_idempotent() {
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    for i in 0..100 {
        let key = format!("key_{:03}", i);
        trie.insert(key.into_bytes(), i).unwrap();
    }
    trie.optimize();
    let arena_len_1 = trie.arena.len();
    trie.optimize();
    let arena_len_2 = trie.arena.len();
    assert_eq!(arena_len_1, arena_len_2, "second optimize changed arena size");
    for i in 0..100 {
        let key = format!("key_{:03}", i);
        assert!(trie.get(key.as_bytes()).is_some());
    }
}

#[test]
fn optimize_byte_boundary_keys() {
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    let mut indices = Vec::new();
    for b in 1u8..=255 {
        let idx = trie.insert(vec![b], b as i32).unwrap();
        indices.push(idx);
    }
    trie.optimize();
    for (i, b) in (1u8..=255).enumerate() {
        let key = vec![b];
        assert_eq!(trie.get(&key), Some(indices[i]),
            "lookup failed after optimize for byte {}", b);
    }
}

#[test]
fn optimize_stress_1000() {
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    let mut indices = Vec::new();
    for i in 0..1000u32 {
        let key = format!("key_{:05}", i);
        let idx = trie.insert(key.into_bytes(), i as i32).unwrap();
        indices.push(idx);
    }
    trie.optimize();
    for i in 0..1000u32 {
        let key = format!("key_{:05}", i);
        assert_eq!(trie.get(key.as_bytes()), Some(indices[i as usize]),
            "lookup failed after optimize at i={}", i);
    }
}

#[test]
fn optimize_deeply_nested() {
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    let mut key = Vec::new();
    let mut indices = Vec::new();
    for i in 0..100 {
        key.push(b'a');
        let idx = trie.insert(key.clone(), i).unwrap();
        indices.push(idx);
    }
    trie.optimize();
    for i in 0..100 {
        let key = vec![b'a'; i + 1];
        assert_eq!(trie.get(&key), Some(indices[i]));
    }
}

#[test]
fn optimize_sorts_buf() {
    // After optimize(), keys in buf should appear in contiguous sorted order.
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    for i in (0..100u32).rev() {
        let key = format!("key_{:05}", i);
        trie.insert(key.into_bytes(), i as i32).unwrap();
    }
    trie.optimize();

    // Iterate in forward order and verify keys are sorted and contiguous in buf
    let mut it = trie.iter();
    let mut prev_key: Option<Vec<u8>> = None;
    let mut prev_end: usize = 1; // first key starts after dummy byte
    while let Some((k, _)) = it.current() {
        if let Some(ref pk) = prev_key {
            assert!(pk.as_slice() <= k, "keys not sorted: {:?} > {:?}",
                std::str::from_utf8(pk), std::str::from_utf8(k));
        }
        let ki = it.current_index().unwrap();
        let (off, len) = trie.index[ki];
        assert_eq!(off, prev_end,
            "key {:?} not contiguous: expected offset {}, got {}",
            std::str::from_utf8(k), prev_end, off);
        prev_key = Some(k.to_vec());
        prev_end = off + len.as_usize();
        it.next();
    }
}

#[test]
fn optimize_sorts_index_and_values() {
    // After optimize(), index[i] and values[i-1] should be in DFS (sorted) order.
    // Insert keys in reverse order to ensure the sort actually changes something.
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    let n = 100;
    for i in (0..n).rev() {
        let key = format!("key_{:05}", i);
        trie.insert(key.into_bytes(), i as i32).unwrap();
    }
    trie.optimize();

    // Verify that index entries are in sorted key order
    for i in 1..trie.index.len() - 1 {
        let (off1, len1) = trie.index[i];
        let (off2, len2) = trie.index[i + 1];
        let key1 = &trie.buf[off1..off1 + len1.as_usize()];
        let key2 = &trie.buf[off2..off2 + len2.as_usize()];
        assert!(key1 <= key2,
            "index not sorted at position {}: {:?} > {:?}",
            i, std::str::from_utf8(key1), std::str::from_utf8(key2));
    }

    // Verify values match their keys
    for i in 0..n {
        let ki = i + 1;
        let (off, len) = trie.index[ki];
        let key = &trie.buf[off..off + len.as_usize()];
        // Keys are key_00000..key_00099, and value == last 5 digits
        let expected_val = std::str::from_utf8(key).unwrap()
            .strip_prefix("key_").unwrap().parse::<i32>().unwrap();
        assert_eq!(trie.values[ki - 1], expected_val,
            "value mismatch at index {}: got {}, expected {}",
            ki, trie.values[ki - 1], expected_val);
    }
}

#[test]
fn optimize_into_keys_values_sorted() {
    // into_keys_values() should return keys in sorted order after optimize
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    for i in (0..50u32).rev() {
        let key = format!("key_{:05}", i);
        trie.insert(key.into_bytes(), i as i32).unwrap();
    }
    trie.optimize();

    let (keys, values) = trie.into_keys_values();
    assert_eq!(keys.len(), 50);
    for i in 1..keys.len() {
        assert!(keys[i] > keys[i - 1], "keys not sorted at index {}", i);
    }
    // Values should match their keys: key_00000 has value 0, etc.
    for i in 0..50 {
        let expected = i as i32;
        assert_eq!(values[i], expected, "value mismatch at index {}", i);
    }
}

#[test]
fn iter_forward_prefix_keys() {
    // "ab" < "abc" in forward order
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"ab".to_vec(), 2).unwrap();
    trie.insert(b"abd".to_vec(), 3).unwrap();

    let mut results = Vec::new();
    let mut iter = trie.iter();
    if let Some((k, _)) = iter.current() { results.push(k.to_vec()); }
    while let Some((k, _)) = iter.next() { results.push(k.to_vec()); }
    assert_eq!(results, vec![b"ab".to_vec(), b"abc".to_vec(), b"abd".to_vec()]);
}

#[test]
fn iter_backward_prefix_keys() {
    // "abd" > "abc" > "ab" in backward order
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"ab".to_vec(), 2).unwrap();
    trie.insert(b"abd".to_vec(), 3).unwrap();

    let mut iter = trie.iter_last();
    let mut results = Vec::new();
    loop {
        match iter.current() {
            Some((k, _)) => results.push(k.to_vec()),
            None => break,
        }
        if iter.prev().is_none() { break; }
    }
    assert_eq!(results, vec![b"abd".to_vec(), b"abc".to_vec(), b"ab".to_vec()]);
}

#[test]
fn iter_forward_empty_key() {
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    trie.insert(b"".to_vec(), 0).unwrap();
    trie.insert(b"abc".to_vec(), 1).unwrap();

    let mut results = Vec::new();
    let mut iter = trie.iter();
    if let Some((k, _)) = iter.current() { results.push(k.to_vec()); }
    while let Some((k, _)) = iter.next() { results.push(k.to_vec()); }
    assert_eq!(results, vec![b"".to_vec(), b"abc".to_vec()]);
}

#[test]
fn optimize_preserves_terminal_flags() {
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    trie.insert(b"ab".to_vec(), 1).unwrap();
    trie.insert(b"abcd".to_vec(), 2).unwrap();
    trie.optimize();
    assert_eq!(trie.get_value(b"ab"), Some(&1), "terminal key 'ab' lost after optimize");
    assert_eq!(trie.get_value(b"abcd"), Some(&2));
    assert_eq!(trie.len(), 2);

    // Also test reverse order
    let mut trie2: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    trie2.insert(b"abcd".to_vec(), 1).unwrap();
    trie2.insert(b"ab".to_vec(), 2).unwrap();
    trie2.optimize();
    assert_eq!(trie2.get_value(b"abcd"), Some(&1));
    assert_eq!(trie2.get_value(b"ab"), Some(&2), "terminal key 'ab' lost after optimize (reverse insert)");
}

#[test]
fn null_bytes_in_keys() {
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    let i1 = trie.insert(b"a\0b".to_vec(), 1).unwrap();
    let i2 = trie.insert(b"a\0c".to_vec(), 2).unwrap();
    let i3 = trie.insert(b"\0".to_vec(), 3).unwrap();
    let i4 = trie.insert(b"\0\0".to_vec(), 4).unwrap();

    assert_eq!(trie.get(b"a\0b"), Some(i1));
    assert_eq!(trie.get(b"a\0c"), Some(i2));
    assert_eq!(trie.get(b"\0"), Some(i3));
    assert_eq!(trie.get(b"\0\0"), Some(i4));
    assert_eq!(trie.len(), 4);
}

// ── Compact mode tests (u16/u16) ──────────────────────────────────

#[test]
fn compact_insert_and_get() {
    let mut trie: NibbleTrie<Vec<u8>, i32, u16, u16> = NibbleTrie::new();
    let idx = trie.insert(b"hello".to_vec(), 42).unwrap();
    assert_eq!(trie.get(b"hello"), Some(idx));
    assert_eq!(trie.get(b"world"), None);
}

#[test]
fn compact_insert_prefix_keys() {
    let mut trie: NibbleTrie<Vec<u8>, i32, u16, u16> = NibbleTrie::new();
    let i1 = trie.insert(b"abc".to_vec(), 1).unwrap();
    let i2 = trie.insert(b"abcd".to_vec(), 2).unwrap();
    assert_eq!(trie.get(b"abc"), Some(i1));
    assert_eq!(trie.get(b"abcd"), Some(i2));
}

#[test]
fn compact_iter_forward() {
    let mut trie: NibbleTrie<Vec<u8>, i32, u16, u16> = NibbleTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"abd".to_vec(), 2).unwrap();
    trie.insert(b"abe".to_vec(), 3).unwrap();

    let mut results = Vec::new();
    let mut iter = trie.iter();
    while let Some((k, _)) = iter.next() {
        results.push(k.to_vec());
    }
    assert_eq!(results, vec![b"abc", b"abd", b"abe"]);
}

#[test]
fn compact_iter_backward() {
    let mut trie: NibbleTrie<Vec<u8>, i32, u16, u16> = NibbleTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"abd".to_vec(), 2).unwrap();
    trie.insert(b"abe".to_vec(), 3).unwrap();

    let mut iter = trie.iter_last();
    let mut results = Vec::new();
    loop {
        match iter.current() {
            Some((k, _)) => results.push(k.to_vec()),
            None => break,
        }
        if iter.prev().is_none() { break; }
    }
    assert_eq!(results, vec![b"abe", b"abd", b"abc"]);
}

#[test]
fn compact_optimize() {
    let mut trie: NibbleTrie<Vec<u8>, i32, u16, u16> = NibbleTrie::new();
    let mut indices = Vec::new();
    for i in 0..100 {
        let key = format!("key_{:03}", i);
        let idx = trie.insert(key.into_bytes(), i).unwrap();
        indices.push(idx);
    }
    trie.optimize();
    for i in 0..100 {
        let key = format!("key_{:03}", i);
        assert_eq!(trie.get(key.as_bytes()), Some(indices[i]),
            "compact lookup failed after optimize for i={}", i);
    }
}

#[test]
fn compact_seek() {
    let mut trie: NibbleTrie<Vec<u8>, i32, u16, u16> = NibbleTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"abd".to_vec(), 2).unwrap();
    trie.insert(b"abe".to_vec(), 3).unwrap();

    let mut iter = trie.iter();
    let (k, _) = iter.seek(b"abd").unwrap();
    assert_eq!(k, b"abd");
}

#[test]
fn compact_empty_key() {
    let mut trie: NibbleTrie<Vec<u8>, i32, u16, u16> = NibbleTrie::new();
    trie.insert(b"".to_vec(), 0).unwrap();
    trie.insert(b"abc".to_vec(), 1).unwrap();

    let mut results = Vec::new();
    let mut iter = trie.iter();
    if let Some((k, _)) = iter.current() { results.push(k.to_vec()); }
    while let Some((k, _)) = iter.next() { results.push(k.to_vec()); }
    assert_eq!(results, vec![b"".to_vec(), b"abc".to_vec()]);
}

// ── TrieIndex trait tests ─────────────────────────────────────────

#[test]
fn trie_index_as_usize() {
    assert_eq!(u16::as_usize(42u16), 42);
    assert_eq!(u32::as_usize(42u32), 42);
    assert_eq!(u64::as_usize(42u64), 42);
    assert_eq!(u16::as_usize(u16::MAX), u16::MAX as usize);
    assert_eq!(u32::as_usize(u32::MAX), u32::MAX as usize);
}

#[test]
fn trie_index_max_value() {
    assert_eq!(<u16 as TrieIndex>::max_value(), u16::MAX as usize);
    assert_eq!(<u32 as TrieIndex>::max_value(), u32::MAX as usize);
    assert_eq!(<u64 as TrieIndex>::max_value(), u64::MAX as usize);
}

#[test]
fn trie_index_from_usize() {
    assert_eq!(u16::from_usize(42), 42u16);
    assert_eq!(u32::from_usize(42), 42u32);
    assert_eq!(u64::from_usize(42), 42u64);
}

// ── Terminal flag tests ──────────────────────────────────────────

#[test]
fn terminal_flag() {
    let mut node: Node<u32, u16> = Node::new();
    assert!(!node.is_terminal());
    assert_eq!(node.terminal, false);

    node.set_terminal(true);
    assert!(node.is_terminal());
    assert_eq!(node.terminal, true);

    node.set_terminal(false);
    assert!(!node.is_terminal());
    assert_eq!(node.terminal, false);

    // Terminal flag is independent of other node fields
    node.set_leaf_child(3, u32::from_usize(42));
    node.set_terminal(true);
    assert!(node.is_terminal());
    assert!(node.is_occupied(3));
    assert!(node.is_leaf(3));
}

// ── u8 PTR tests ──────────────────────────────────────────────────────

#[test]
fn node_size_u8() {
    // Compact PTR=u8, LEN=u16:
    // [OptNz<u8>;16] (children) + u16 (prefix_len) + u16 (leaf_mask)
    // + OptNz<u8> (leaf) + bool (terminal)
    // = 16 + 2 + 2 + 1 + 1 = 22 bytes (align to u16)
    assert_eq!(std::mem::size_of::<Node<u8, u16>>(), 22);
}

#[test]
fn u8_insert_and_get() {
    let mut trie: NibbleTrie<Vec<u8>, i32, u8, u16> = NibbleTrie::new();
    let idx = trie.insert(b"hello".to_vec(), 42).unwrap();
    assert_eq!(trie.get(b"hello"), Some(idx));
    assert_eq!(trie.get(b"world"), None);
}

#[test]
fn u8_near_capacity() {
    let mut trie: NibbleTrie<Vec<u8>, i32, u8, u16> = NibbleTrie::new();
    // near_capacity when arena.len() >= u8::MAX or index.len() >= u8::MAX (255).
    assert!(!trie.near_capacity());
    // Insert some keys — should not be near capacity yet
    for i in 0..50u32 {
        let key = format!("k{:02}", i);
        trie.insert(key.into_bytes(), i as i32).unwrap();
    }
    assert!(!trie.near_capacity());
}

#[test]
fn u8_overflow() {
    let mut trie: NibbleTrie<Vec<u8>, i32, u8, u16> = NibbleTrie::new();
    // With u8 PTR, child slots and leaf use OptNz<u8> with 0 as the empty
    // sentinel. Real key indices are 1..=255 (index 0 is the dummy), but
    // near_capacity trips once index.len() reaches 255 — i.e. after 254 real
    // keys — so the 255th insert is rejected.
    for i in 0..254u32 {
        let key = format!("k{:03}", i);
        trie.insert(key.into_bytes(), i as i32).unwrap();
    }
    // Try one more — should fail (index.len() would reach 256 >= 255).
    let result = trie.insert(b"overflow".to_vec(), 999);
    assert_eq!(result, Err(()));
}

// ── promote/demote tests ─────────────────────────────────────────────

#[test]
fn promote_u8_to_u16() {
    let mut trie: NibbleTrie<Vec<u8>, i32, u8, u16> = NibbleTrie::new();
    let mut indices = Vec::new();
    for i in 0..50u32 {
        let key = format!("key_{:03}", i);
        let idx = trie.insert(key.into_bytes(), i as i32).unwrap();
        indices.push(idx);
    }
    let promoted: NibbleTrie<Vec<u8>, i32, u16, u16> = trie.promote::<u16>();
    // All lookups still work after promotion
    for i in 0..50u32 {
        let key = format!("key_{:03}", i);
        assert_eq!(promoted.get(key.as_bytes()), Some(indices[i as usize]),
            "lookup failed after promote for i={}", i);
    }
    assert_eq!(promoted.len(), 50);
}

#[test]
fn promote_u16_to_u32() {
    let mut trie: NibbleTrie<Vec<u8>, i32, u16, u16> = NibbleTrie::new();
    let mut indices = Vec::new();
    for i in 0..100u32 {
        let key = format!("key_{:03}", i);
        let idx = trie.insert(key.into_bytes(), i as i32).unwrap();
        indices.push(idx);
    }
    let promoted: NibbleTrie<Vec<u8>, i32, u32, u16> = trie.promote::<u32>();
    for i in 0..100u32 {
        let key = format!("key_{:03}", i);
        assert_eq!(promoted.get(key.as_bytes()), Some(indices[i as usize]));
    }
}

#[test]
fn demote_u16_to_u8() {
    let mut trie: NibbleTrie<Vec<u8>, i32, u16, u16> = NibbleTrie::new();
    let mut indices = Vec::new();
    for i in 0..10u32 {
        let key = format!("key_{:03}", i);
        let idx = trie.insert(key.into_bytes(), i as i32).unwrap();
        indices.push(idx);
    }
    let demoted: NibbleTrie<Vec<u8>, i32, u8, u16> = match trie.demote::<u8>() {
        Ok(d) => d,
        Err(_) => panic!("demote should succeed with 10 keys"),
    };
    for i in 0..10u32 {
        let key = format!("key_{:03}", i);
        assert_eq!(demoted.get(key.as_bytes()), Some(indices[i as usize]));
    }
}

#[test]
fn demote_fails_too_large() {
    let mut trie: NibbleTrie<Vec<u8>, i32, u16, u16> = NibbleTrie::new();
    // Insert more than u8::MAX keys — can't demote to u8
    for i in 0..300u32 {
        let key = format!("key_{:05}", i);
        trie.insert(key.into_bytes(), i as i32).unwrap();
    }
    let result = trie.demote::<u8>();
    assert!(result.is_err(), "demote should fail when trie is too large");
}


// ── Generic key type tests ──────────────────────────────────────────

#[test]
fn string_key_insert_and_get() {
    let mut trie: NibbleTrie<String, i32> = NibbleTrie::new();
    trie.insert("hello".to_string(), 1).unwrap();
    trie.insert("world".to_string(), 2).unwrap();
    assert_eq!(trie.get(b"hello"), Some(1));
    assert_eq!(trie.get(b"world"), Some(2));
    assert_eq!(trie.get(b"hell"), None);
}

#[test]
fn string_key_into_keys_values() {
    let mut trie: NibbleTrie<String, i32> = NibbleTrie::new();
    trie.insert("abc".to_string(), 1).unwrap();
    trie.insert("def".to_string(), 2).unwrap();
    let (keys, values) = trie.into_keys_values();
    assert_eq!(keys, vec!["abc".to_string(), "def".to_string()]);
    assert_eq!(values, vec![1, 2]);
}

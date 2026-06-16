use super::*;

#[test]
fn node_size_default() {
    // Default PTR_T=u32, LEN_T=u16, STAK=1 should be 76 bytes
    assert_eq!(std::mem::size_of::<Node<u32, u16>>(), 76);
}

#[test]
fn node_size_compact() {
    // Compact PTR_T=u16, LEN_T=u16, STAK=1 should be 42 bytes
    assert_eq!(std::mem::size_of::<Node<u16, u16>>(), 42);
}

#[test]
fn insert_empty_and_get() {
    let mut trie: NibbleTrie<i32> = NibbleTrie::new();
    let idx = trie.insert(b"hello".to_vec(), 42).unwrap();
    assert_eq!(trie.get(b"hello"), Some(idx));
    assert_eq!(trie.get_value(b"hello"), Some(&42));
    assert_eq!(trie.get(b"world"), None);
}

#[test]
fn insert_duplicate_returns_error() {
    let mut trie: NibbleTrie<i32> = NibbleTrie::new();
    trie.insert(b"hello".to_vec(), 1).unwrap();
    let result = trie.insert(b"hello".to_vec(), 2);
    assert_eq!(result, Err(()));
    assert_eq!(trie.len(), 1);
}

#[test]
fn insert_null_byte_allowed() {
    let mut trie: NibbleTrie<i32> = NibbleTrie::new();
    // Null bytes are now valid in keys
    let idx = trie.insert(b"hel\0lo".to_vec(), 1).unwrap();
    assert_eq!(trie.get(b"hel\0lo"), Some(idx));
}

#[test]
fn insert_two_keys_split_leaf() {
    let mut trie: NibbleTrie<i32> = NibbleTrie::new();
    let i1 = trie.insert(b"abc".to_vec(), 1).unwrap();
    let i2 = trie.insert(b"abd".to_vec(), 2).unwrap();
    assert_eq!(trie.get(b"abc"), Some(i1));
    assert_eq!(trie.get(b"abd"), Some(i2));
    assert_eq!(trie.len(), 2);
}

#[test]
fn insert_prefix_key() {
    let mut trie: NibbleTrie<i32> = NibbleTrie::new();
    let i1 = trie.insert(b"abc".to_vec(), 1).unwrap();
    let i2 = trie.insert(b"abcd".to_vec(), 2).unwrap();
    assert_eq!(trie.get(b"abc"), Some(i1));
    assert_eq!(trie.get(b"abcd"), Some(i2));
}

#[test]
fn insert_reverse_prefix_key() {
    let mut trie: NibbleTrie<i32> = NibbleTrie::new();
    let i1 = trie.insert(b"abcd".to_vec(), 1).unwrap();
    let i2 = trie.insert(b"abc".to_vec(), 2).unwrap();
    assert_eq!(trie.get(b"abcd"), Some(i1));
    assert_eq!(trie.get(b"abc"), Some(i2));
}

#[test]
fn insert_no_common_prefix() {
    let mut trie: NibbleTrie<i32> = NibbleTrie::new();
    let i1 = trie.insert(b"abc".to_vec(), 1).unwrap();
    let i2 = trie.insert(b"xyz".to_vec(), 2).unwrap();
    assert_eq!(trie.get(b"abc"), Some(i1));
    assert_eq!(trie.get(b"xyz"), Some(i2));
}

#[test]
fn insert_three_keys() {
    let mut trie: NibbleTrie<i32> = NibbleTrie::new();
    let i1 = trie.insert(b"abc".to_vec(), 1).unwrap();
    let i2 = trie.insert(b"abd".to_vec(), 2).unwrap();
    let i3 = trie.insert(b"abe".to_vec(), 3).unwrap();
    assert_eq!(trie.get(b"abc"), Some(i1));
    assert_eq!(trie.get(b"abd"), Some(i2));
    assert_eq!(trie.get(b"abe"), Some(i3));
}

#[test]
fn insert_single_char_keys() {
    let mut trie: NibbleTrie<i32> = NibbleTrie::new();
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
    let mut trie: NibbleTrie<i32> = NibbleTrie::new();
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
    let mut trie: NibbleTrie<i32> = NibbleTrie::new();
    let mut key = Vec::new();
    for i in 0..100 {
        key.push(b'a');
        let idx = trie.insert(key.clone(), i).unwrap();
        assert_eq!(trie.get(&key), Some(idx));
    }
}

#[test]
fn len_and_is_empty() {
    let mut trie: NibbleTrie<i32> = NibbleTrie::new();
    assert!(trie.is_empty());
    assert_eq!(trie.len(), 0);
    trie.insert(b"hello".to_vec(), 1).unwrap();
    assert!(!trie.is_empty());
    assert_eq!(trie.len(), 1);
}

#[test]
fn into_keys_values_roundtrip() {
    let mut trie: NibbleTrie<i32> = NibbleTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"def".to_vec(), 2).unwrap();
    let (keys, values) = trie.into_keys_values();
    assert_eq!(keys, vec![b"abc".to_vec(), b"def".to_vec()]);
    assert_eq!(values, vec![1, 2]);
}

#[test]
fn iter_empty() {
    let trie: NibbleTrie<i32> = NibbleTrie::new();
    let mut iter = trie.iter();
    assert!(iter.next().is_none());
}

#[test]
fn iter_single_key() {
    let mut trie: NibbleTrie<i32> = NibbleTrie::new();
    trie.insert(b"hello".to_vec(), 42).unwrap();
    let mut iter = trie.iter();
    let (k, v) = iter.next().unwrap();
    assert_eq!(k, b"hello");
    assert_eq!(*v, 42);
    assert!(iter.next().is_none());
}

#[test]
fn iter_forward() {
    let mut trie: NibbleTrie<i32> = NibbleTrie::new();
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
    let mut trie: NibbleTrie<i32> = NibbleTrie::new();
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
    let mut trie: NibbleTrie<i32> = NibbleTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"abd".to_vec(), 2).unwrap();
    trie.insert(b"abe".to_vec(), 3).unwrap();

    let mut iter = trie.iter();
    let (k, _) = iter.seek(b"abd").unwrap();
    assert_eq!(k, b"abd");
}

#[test]
fn iter_seek_between() {
    let mut trie: NibbleTrie<i32> = NibbleTrie::new();
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
    let mut trie: NibbleTrie<i32> = NibbleTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"abcd".to_vec(), 2).unwrap();

    let mut iter = trie.iter();
    let (k, _) = iter.seek(b"abc").unwrap();
    assert_eq!(k, b"abc");
}

#[test]
fn get_value_found_and_missing() {
    let mut trie: NibbleTrie<String> = NibbleTrie::new();
    trie.insert(b"hello".to_vec(), "world".to_string()).unwrap();
    assert_eq!(trie.get_value(b"hello"), Some(&"world".to_string()));
    assert_eq!(trie.get_value(b"world"), None);
}

#[test]
fn iter_backward_large() {
    let mut trie: NibbleTrie<i32> = NibbleTrie::new();
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
    let mut trie: NibbleTrie<i32> = NibbleTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    // Root should have leaf field set (not the sentinel)
    let root = &trie.arena[0];
    assert_ne!(root.leaf, u32::max_value_sentinel(), "root leaf field should be set");
    // offset should point to the key in buf (stored in index, not node)
    let (off, len) = trie.index[root.leaf.as_usize()];
    assert_ne!(off, 0, "root offset should be set");
    assert_eq!(&trie.buf[off..off + len.as_usize()], b"abc");
}

// ── optimize() tests ──────────────────────────────────────────────

#[test]
fn optimize_empty() {
    let mut trie: NibbleTrie<i32> = NibbleTrie::new();
    trie.optimize();
    assert!(trie.is_empty());
}

#[test]
fn optimize_single_key() {
    let mut trie: NibbleTrie<i32> = NibbleTrie::new();
    let idx = trie.insert(b"hello".to_vec(), 42).unwrap();
    trie.optimize();
    assert_eq!(trie.get(b"hello"), Some(idx));
    assert_eq!(trie.len(), 1);
}

#[test]
fn optimize_preserves_lookups() {
    let mut trie: NibbleTrie<i32> = NibbleTrie::new();
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
    let mut trie: NibbleTrie<i32> = NibbleTrie::new();
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
    let mut trie: NibbleTrie<i32> = NibbleTrie::new();
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
    let mut trie: NibbleTrie<i32> = NibbleTrie::new();
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
    let mut trie: NibbleTrie<i32> = NibbleTrie::new();
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
    let mut trie: NibbleTrie<i32> = NibbleTrie::new();
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
    let mut trie: NibbleTrie<i32> = NibbleTrie::new();
    let mut key = Vec::new();
    let mut indices = Vec::new();
    for i in 0..100 {
        key.push(b'a');
        let idx = trie.insert(key.clone(), i).unwrap();
        indices.push(idx);
    }
    trie.optimize();
    for i in 0..100 {
        let mut key = vec![b'a'; i + 1];
        assert_eq!(trie.get(&key), Some(indices[i]));
    }
}

#[test]
fn optimize_sorts_buf() {
    // After optimize(), keys in buf should appear in contiguous sorted order.
    let mut trie: NibbleTrie<i32> = NibbleTrie::new();
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
    let mut trie: NibbleTrie<i32> = NibbleTrie::new();
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
    let mut trie: NibbleTrie<i32> = NibbleTrie::new();
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
    let mut trie: NibbleTrie<i32> = NibbleTrie::new();
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
    let mut trie: NibbleTrie<i32> = NibbleTrie::new();
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
    let mut trie: NibbleTrie<i32> = NibbleTrie::new();
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
    let mut trie: NibbleTrie<i32> = NibbleTrie::new();
    trie.insert(b"ab".to_vec(), 1).unwrap();
    trie.insert(b"abcd".to_vec(), 2).unwrap();
    trie.optimize();
    assert_eq!(trie.get_value(b"ab"), Some(&1), "terminal key 'ab' lost after optimize");
    assert_eq!(trie.get_value(b"abcd"), Some(&2));
    assert_eq!(trie.len(), 2);

    // Also test reverse order
    let mut trie2: NibbleTrie<i32> = NibbleTrie::new();
    trie2.insert(b"abcd".to_vec(), 1).unwrap();
    trie2.insert(b"ab".to_vec(), 2).unwrap();
    trie2.optimize();
    assert_eq!(trie2.get_value(b"abcd"), Some(&1));
    assert_eq!(trie2.get_value(b"ab"), Some(&2), "terminal key 'ab' lost after optimize (reverse insert)");
}

#[test]
fn null_bytes_in_keys() {
    let mut trie: NibbleTrie<i32> = NibbleTrie::new();
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
    let mut trie: NibbleTrie<i32, u16, u16> = NibbleTrie::new();
    let idx = trie.insert(b"hello".to_vec(), 42).unwrap();
    assert_eq!(trie.get(b"hello"), Some(idx));
    assert_eq!(trie.get(b"world"), None);
}

#[test]
fn compact_insert_prefix_keys() {
    let mut trie: NibbleTrie<i32, u16, u16> = NibbleTrie::new();
    let i1 = trie.insert(b"abc".to_vec(), 1).unwrap();
    let i2 = trie.insert(b"abcd".to_vec(), 2).unwrap();
    assert_eq!(trie.get(b"abc"), Some(i1));
    assert_eq!(trie.get(b"abcd"), Some(i2));
}

#[test]
fn compact_iter_forward() {
    let mut trie: NibbleTrie<i32, u16, u16> = NibbleTrie::new();
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
    let mut trie: NibbleTrie<i32, u16, u16> = NibbleTrie::new();
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
    let mut trie: NibbleTrie<i32, u16, u16> = NibbleTrie::new();
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
    let mut trie: NibbleTrie<i32, u16, u16> = NibbleTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"abd".to_vec(), 2).unwrap();
    trie.insert(b"abe".to_vec(), 3).unwrap();

    let mut iter = trie.iter();
    let (k, _) = iter.seek(b"abd").unwrap();
    assert_eq!(k, b"abd");
}

#[test]
fn compact_empty_key() {
    let mut trie: NibbleTrie<i32, u16, u16> = NibbleTrie::new();
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
    assert!(!node.is_terminal(0));
    assert_eq!(node.terminal, 0);

    node.set_terminal(0, true);
    assert!(node.is_terminal(0));
    assert_eq!(node.terminal, 1);

    node.set_terminal(0, false);
    assert!(!node.is_terminal(0));
    assert_eq!(node.terminal, 0);

    // Terminal flag is independent of other node fields
    node.set_leaf_child(3, 0, u32::from_usize(42));
    node.set_terminal(0, true);
    assert!(node.is_terminal(0));
    assert!(node.is_occupied(3, 0));
    assert!(node.is_leaf(3, 0));
}

// ── u8 PTR tests ──────────────────────────────────────────────────────

#[test]
fn node_size_u8() {
    // Compact PTR=u8, LEN=u16, STAK=1:
    // [u16;1]×3 (prefix_len, leaf_mask, occupancy) + [u8;16] (children) + [u8;1] (leaf) + u8 (nodelen) + u8 (terminal)
    // = 6 + 16 + 1 + 1 + 1 = 25 bytes, padded to 26 (align to u16)
    assert_eq!(std::mem::size_of::<Node<u8, u16>>(), 26);
}

#[test]
fn u8_insert_and_get() {
    let mut trie: NibbleTrie<i32, u8, u16> = NibbleTrie::new();
    let idx = trie.insert(b"hello".to_vec(), 42).unwrap();
    assert_eq!(trie.get(b"hello"), Some(idx));
    assert_eq!(trie.get(b"world"), None);
}

#[test]
fn u8_near_capacity() {
    let mut trie: NibbleTrie<i32, u8, u16> = NibbleTrie::new();
    // u8 max_value_sentinel = 255, near_capacity when arena.len() >= 255 or index.len() >= 255
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
    let mut trie: NibbleTrie<i32, u8, u16> = NibbleTrie::new();
    // With u8 PTR, PTR::MAX (255) is the sentinel for empty children slots.
    // This means valid key indices are 1..=254 (index 0 is dummy, 255 is sentinel).
    // So at most 254 keys can be inserted.
    for i in 0..254u32 {
        let key = format!("k{:03}", i);
        trie.insert(key.into_bytes(), i as i32).unwrap();
    }
    // Try one more — should fail (index 255 = sentinel).
    let result = trie.insert(b"overflow".to_vec(), 999);
    assert_eq!(result, Err(()));
}

// ── promote/demote tests ─────────────────────────────────────────────

#[test]
fn promote_u8_to_u16() {
    let mut trie: NibbleTrie<i32, u8, u16> = NibbleTrie::new();
    let mut indices = Vec::new();
    for i in 0..50u32 {
        let key = format!("key_{:03}", i);
        let idx = trie.insert(key.into_bytes(), i as i32).unwrap();
        indices.push(idx);
    }
    let promoted: NibbleTrie<i32, u16, u16> = trie.promote::<u16>();
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
    let mut trie: NibbleTrie<i32, u16, u16> = NibbleTrie::new();
    let mut indices = Vec::new();
    for i in 0..100u32 {
        let key = format!("key_{:03}", i);
        let idx = trie.insert(key.into_bytes(), i as i32).unwrap();
        indices.push(idx);
    }
    let promoted: NibbleTrie<i32, u32, u16> = trie.promote::<u32>();
    for i in 0..100u32 {
        let key = format!("key_{:03}", i);
        assert_eq!(promoted.get(key.as_bytes()), Some(indices[i as usize]));
    }
}

#[test]
fn demote_u16_to_u8() {
    let mut trie: NibbleTrie<i32, u16, u16> = NibbleTrie::new();
    let mut indices = Vec::new();
    for i in 0..10u32 {
        let key = format!("key_{:03}", i);
        let idx = trie.insert(key.into_bytes(), i as i32).unwrap();
        indices.push(idx);
    }
    let demoted: NibbleTrie<i32, u8, u16> = match trie.demote::<u8>() {
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
    let mut trie: NibbleTrie<i32, u16, u16> = NibbleTrie::new();
    // Insert more than u8::MAX keys — can't demote to u8
    for i in 0..300u32 {
        let key = format!("key_{:05}", i);
        trie.insert(key.into_bytes(), i as i32).unwrap();
    }
    let result = trie.demote::<u8>();
    assert!(result.is_err(), "demote should fail when trie is too large");
}

// ── STAK>1 (node stacking) tests ──────────────────────────────────

/// Helper: build a STAK=1 trie then convert it to STAK=2 by expanding
/// each node into a physical node with vnode 0 holding the original data
/// and vnode 1 empty. This tests the STAK>1 traversal path while keeping
/// the trie structure logically identical.
///
/// Internal child addresses are remapped: STAK=1 addresses are just phys_idx,
/// STAK=2 addresses are phys_idx*2 + vnode_idx. Leaf child values (key indices)
/// stay the same since they're indices into the index/buf arrays, not arena refs.
fn build_stak2_from_stak1<T: Clone>(trie1: &NibbleTrie<T, u32, u16, 1>) -> NibbleTrie<T, u32, u16, 2> {
    let mut trie2: NibbleTrie<T, u32, u16, 2> = NibbleTrie::new();
    trie2.buf = trie1.buf.clone();
    trie2.index = trie1.index.clone();
    trie2.values = trie1.values.clone();

    // Convert each STAK=1 node to a STAK=2 physical node.
    // Vnode 0 keeps all the data; vnode 1 is empty (occupancy=0).
    for node1 in &trie1.arena {
        let mut node2: Node<u32, u16, 2> = Node::new();
        // Copy vnode 0 data, remapping internal child addresses
        for nib in 0..16 {
            if node1.is_occupied(nib, 0) {
                if node1.is_leaf(nib, 0) {
                    // Leaf: value is a key index, no remapping needed
                    node2.children[nib] = node1.children[nib];
                } else {
                    // Internal: value is a phys_idx, remap phys → phys*2
                    node2.children[nib] = u32::from_usize(node1.children[nib].as_usize() * 2);
                }
                node2.occupancy[0] |= 1 << nib;
                if node1.is_leaf(nib, 0) {
                    node2.leaf_mask[0] |= 1 << nib;
                }
            } else {
                // Empty slot — already set to sentinel by Node::new()
            }
        }
        node2.prefix_len[0] = node1.prefix_len[0];
        node2.leaf = node1.leaf;
        node2.terminal = if node1.is_terminal(0) { 1 } else { 0 };
        // nodelen is already 1 from Node::new() — one vnode per physical node in the conversion
        // Vnode 1 is empty — all fields are zero/sentinel from Node::new()
        trie2.arena.push(node2);
    }

    trie2
}

#[test]
fn stak2_node_size() {
    // PTR=u32, LEN=u16, STAK=2:
    // children: 16*4=64, leaf: 4, prefix_len: 2*2=4, leaf_mask: 2*2=4, occupancy: 2*2=4, terminal: 1, nodelen: 1
    // = 82 bytes, padded to 84 (align to u32)
    assert_eq!(std::mem::size_of::<Node<u32, u16, 2>>(), 84);
}

#[test]
fn stak2_node_size_compact() {
    // PTR=u16, LEN=u16, STAK=2:
    // children: 16*2=32, leaf: 2, prefix_len: 2*2=4, leaf_mask: 2*2=4, occupancy: 2*2=4, terminal: 1, nodelen: 1
    // = 47 bytes, padded to 48 (align to u16)
    assert_eq!(std::mem::size_of::<Node<u16, u16, 2>>(), 48);
}

#[test]
fn stak2_simple_get() {
    let mut trie1: NibbleTrie<i32> = NibbleTrie::new();
    let i1 = trie1.insert(b"abc".to_vec(), 1).unwrap();
    let i2 = trie1.insert(b"abd".to_vec(), 2).unwrap();

    let trie2 = build_stak2_from_stak1(&trie1);
    assert_eq!(trie2.get(b"abc"), Some(i1));
    assert_eq!(trie2.get(b"abd"), Some(i2));
    assert_eq!(trie2.get(b"abe"), None);
}

#[test]
fn stak2_get_prefix_keys() {
    let mut trie1: NibbleTrie<i32> = NibbleTrie::new();
    let i1 = trie1.insert(b"ab".to_vec(), 1).unwrap();
    let i2 = trie1.insert(b"abcd".to_vec(), 2).unwrap();

    let trie2 = build_stak2_from_stak1(&trie1);
    assert_eq!(trie2.get(b"ab"), Some(i1));
    assert_eq!(trie2.get(b"abcd"), Some(i2));
}

#[test]
fn stak2_iter_forward() {
    let mut trie1: NibbleTrie<i32> = NibbleTrie::new();
    trie1.insert(b"abc".to_vec(), 1).unwrap();
    trie1.insert(b"abd".to_vec(), 2).unwrap();
    trie1.insert(b"abe".to_vec(), 3).unwrap();

    let trie2 = build_stak2_from_stak1(&trie1);
    let mut results = Vec::new();
    let mut iter = trie2.iter();
    while let Some((k, _)) = iter.next() {
        results.push(k.to_vec());
    }
    assert_eq!(results, vec![b"abc".to_vec(), b"abd".to_vec(), b"abe".to_vec()]);
}

#[test]
fn stak2_iter_backward() {
    let mut trie1: NibbleTrie<i32> = NibbleTrie::new();
    trie1.insert(b"abc".to_vec(), 1).unwrap();
    trie1.insert(b"abd".to_vec(), 2).unwrap();
    trie1.insert(b"abe".to_vec(), 3).unwrap();

    let trie2 = build_stak2_from_stak1(&trie1);
    let mut iter = trie2.iter_last();
    let mut results = Vec::new();
    loop {
        match iter.current() {
            Some((k, _)) => results.push(k.to_vec()),
            None => break,
        }
        if iter.prev().is_none() { break; }
    }
    assert_eq!(results, vec![b"abe".to_vec(), b"abd".to_vec(), b"abc".to_vec()]);
}

#[test]
fn stak2_iter_seek() {
    let mut trie1: NibbleTrie<i32> = NibbleTrie::new();
    trie1.insert(b"abc".to_vec(), 1).unwrap();
    trie1.insert(b"abd".to_vec(), 2).unwrap();
    trie1.insert(b"abe".to_vec(), 3).unwrap();

    let trie2 = build_stak2_from_stak1(&trie1);
    let mut iter = trie2.iter();
    let (k, _) = iter.seek(b"abd").unwrap();
    assert_eq!(k, b"abd");
}

#[test]
fn stak2_many_keys() {
    let mut trie1: NibbleTrie<i32> = NibbleTrie::new();
    let mut indices = Vec::new();
    for i in 0..100 {
        let key = format!("key_{:03}", i);
        let idx = trie1.insert(key.into_bytes(), i).unwrap();
        indices.push(idx);
    }

    let trie2 = build_stak2_from_stak1(&trie1);
    for i in 0..100 {
        let key = format!("key_{:03}", i);
        assert_eq!(trie2.get(key.as_bytes()), Some(indices[i]),
            "stak2 lookup failed for i={}", i);
    }
    assert_eq!(trie2.len(), 100);
}

#[test]
fn stak2_iter_forward_large() {
    let mut trie1: NibbleTrie<i32> = NibbleTrie::new();
    for i in 0..100 {
        let key = format!("key_{:03}", i);
        trie1.insert(key.into_bytes(), i).unwrap();
    }

    let trie2 = build_stak2_from_stak1(&trie1);
    let mut results = Vec::new();
    let mut it = trie2.iter();
    while let Some((k, _)) = it.next() {
        results.push(k.to_vec());
    }
    assert_eq!(results.len(), 100);
    for i in 1..results.len() {
        assert!(results[i] > results[i - 1], "not sorted at index {}", i);
    }
}

#[test]
fn stak2_iter_backward_large() {
    let mut trie1: NibbleTrie<i32> = NibbleTrie::new();
    for i in 0..100 {
        let key = format!("key_{:03}", i);
        trie1.insert(key.into_bytes(), i).unwrap();
    }

    let trie2 = build_stak2_from_stak1(&trie1);
    let mut it = trie2.iter_last();
    let mut count = 0;
    let mut last_key: Vec<u8> = Vec::new();
    if let Some((k, _)) = it.current() {
        last_key = k.to_vec();
        count += 1;
    }
    while let Some((k, _)) = it.prev() {
        assert!(k < &last_key[..], "not descending: {:?} >= {:?}",
            String::from_utf8_lossy(k), String::from_utf8_lossy(&last_key));
        last_key = k.to_vec();
        count += 1;
    }
    assert_eq!(count, 100);
}

#[test]
fn stak2_null_bytes_in_keys() {
    let mut trie1: NibbleTrie<i32> = NibbleTrie::new();
    let i1 = trie1.insert(b"a\0b".to_vec(), 1).unwrap();
    let i2 = trie1.insert(b"a\0c".to_vec(), 2).unwrap();
    let i3 = trie1.insert(b"\0".to_vec(), 3).unwrap();
    let i4 = trie1.insert(b"\0\0".to_vec(), 4).unwrap();

    let trie2 = build_stak2_from_stak1(&trie1);
    assert_eq!(trie2.get(b"a\0b"), Some(i1));
    assert_eq!(trie2.get(b"a\0c"), Some(i2));
    assert_eq!(trie2.get(b"\0"), Some(i3));
    assert_eq!(trie2.get(b"\0\0"), Some(i4));
}

#[test]
fn stak2_deeply_nested() {
    let mut trie1: NibbleTrie<i32> = NibbleTrie::new();
    let mut key = Vec::new();
    let mut indices = Vec::new();
    for i in 0..100 {
        key.push(b'a');
        let idx = trie1.insert(key.clone(), i).unwrap();
        indices.push(idx);
    }

    let trie2 = build_stak2_from_stak1(&trie1);
    for i in 0..100 {
        let key = vec![b'a'; i + 1];
        assert_eq!(trie2.get(&key), Some(indices[i]));
    }
}

#[test]
fn stak2_single_key() {
    let mut trie1: NibbleTrie<i32> = NibbleTrie::new();
    let idx = trie1.insert(b"hello".to_vec(), 42).unwrap();

    let trie2 = build_stak2_from_stak1(&trie1);
    assert_eq!(trie2.get(b"hello"), Some(idx));
    assert_eq!(trie2.get(b"world"), None);
    assert_eq!(trie2.len(), 1);
}

#[test]
fn stak2_empty_key() {
    let mut trie1: NibbleTrie<i32> = NibbleTrie::new();
    let i1 = trie1.insert(b"".to_vec(), 0).unwrap();
    let i2 = trie1.insert(b"abc".to_vec(), 1).unwrap();

    let trie2 = build_stak2_from_stak1(&trie1);
    assert_eq!(trie2.get(b""), Some(i1));
    assert_eq!(trie2.get(b"abc"), Some(i2));
}

#[test]
fn stak2_iter_prefix_keys() {
    let mut trie1: NibbleTrie<i32> = NibbleTrie::new();
    trie1.insert(b"abc".to_vec(), 1).unwrap();
    trie1.insert(b"ab".to_vec(), 2).unwrap();
    trie1.insert(b"abd".to_vec(), 3).unwrap();

    let trie2 = build_stak2_from_stak1(&trie1);
    let mut results = Vec::new();
    let mut iter = trie2.iter();
    if let Some((k, _)) = iter.current() { results.push(k.to_vec()); }
    while let Some((k, _)) = iter.next() { results.push(k.to_vec()); }
    assert_eq!(results, vec![b"ab".to_vec(), b"abc".to_vec(), b"abd".to_vec()]);
}
// ── STAK>1 optimize tests ─────────────────────────────────────────

#[test]
fn stak2_optimize_preserves_lookups() {
    let mut trie1: NibbleTrie<i32> = NibbleTrie::new();
    let mut indices = Vec::new();
    for i in 0..100 {
        let key = format!("key_{:03}", i);
        let idx = trie1.insert(key.into_bytes(), i).unwrap();
        indices.push(idx);
    }
    let mut trie2 = build_stak2_from_stak1(&trie1);
    trie2.optimize();
    for i in 0..100 {
        let key = format!("key_{:03}", i);
        assert_eq!(trie2.get(key.as_bytes()), Some(indices[i]),
            "stak2 lookup failed after optimize for i={}", i);
    }
    assert_eq!(trie2.len(), 100);
}

#[test]
fn stak2_optimize_preserves_iteration() {
    let mut trie1: NibbleTrie<i32> = NibbleTrie::new();
    for i in 0..100 {
        let key = format!("key_{:05}", i);
        trie1.insert(key.into_bytes(), i as i32).unwrap();
    }
    let mut trie2 = build_stak2_from_stak1(&trie1);
    trie2.optimize();

    let mut keys: Vec<Vec<u8>> = Vec::new();
    let mut it = trie2.iter();
    while let Some((k, _)) = it.next() {
        keys.push(k.to_vec());
    }
    assert_eq!(keys.len(), 100);
    for i in 1..keys.len() {
        assert!(keys[i] > keys[i - 1], "not sorted after optimize at index {}", i);
    }
}

#[test]
fn stak2_optimize_preserves_seek() {
    let mut trie1: NibbleTrie<i32> = NibbleTrie::new();
    for i in 0..50u32 {
        let key = format!("key_{:05}", i);
        trie1.insert(key.into_bytes(), i as i32).unwrap();
    }
    let mut trie2 = build_stak2_from_stak1(&trie1);
    trie2.optimize();
    let mut it = trie2.iter();
    let (k, v) = it.seek(b"key_00025").unwrap();
    assert_eq!(k, b"key_00025");
    assert_eq!(*v, 25);
}

#[test]
fn stak2_optimize_prefix_keys() {
    let mut trie1: NibbleTrie<i32> = NibbleTrie::new();
    let i1 = trie1.insert(b"ab".to_vec(), 1).unwrap();
    let i2 = trie1.insert(b"abcd".to_vec(), 2).unwrap();
    let mut trie2 = build_stak2_from_stak1(&trie1);
    trie2.optimize();
    assert_eq!(trie2.get(b"ab"), Some(i1));
    assert_eq!(trie2.get(b"abcd"), Some(i2));
}

#[test]
fn stak2_optimize_null_bytes() {
    let mut trie1: NibbleTrie<i32> = NibbleTrie::new();
    let i1 = trie1.insert(b"a\0b".to_vec(), 1).unwrap();
    let i2 = trie1.insert(b"a\0c".to_vec(), 2).unwrap();
    let mut trie2 = build_stak2_from_stak1(&trie1);
    trie2.optimize();
    assert_eq!(trie2.get(b"a\0b"), Some(i1));
    assert_eq!(trie2.get(b"a\0c"), Some(i2));
}

#[test]
fn stak2_optimize_empty_key() {
    let mut trie1: NibbleTrie<i32> = NibbleTrie::new();
    let i1 = trie1.insert(b"".to_vec(), 0).unwrap();
    let i2 = trie1.insert(b"abc".to_vec(), 1).unwrap();
    let mut trie2 = build_stak2_from_stak1(&trie1);
    trie2.optimize();
    assert_eq!(trie2.get(b""), Some(i1));
    assert_eq!(trie2.get(b"abc"), Some(i2));
}

#[test]
fn stak2_optimize_reduces_arena() {
    let mut trie1: NibbleTrie<i32> = NibbleTrie::new();
    for i in 0..50 {
        let key = format!("prefix_{:02}", i);
        trie1.insert(key.into_bytes(), i).unwrap();
    }
    let arena_size_before = trie1.arena.len();
    let mut trie2 = build_stak2_from_stak1(&trie1);
    trie2.optimize();
    assert!(trie2.arena.len() <= arena_size_before,
        "arena grew after optimize: {} -> {}", arena_size_before, trie2.arena.len());
}

// ── Debug test for optimize STAK>1 with diverse keys ────────────────

#[test]
fn stak2_optimize_debug_diverse_keys() {
    // Build a STAK=1 trie with diverse keys (similar to wikipedia words)
    let mut trie1: NibbleTrie<i32> = NibbleTrie::new();
    let words = [
        "the", "be", "to", "of", "and", "a", "in", "that", "have", "I",
        "it", "for", "not", "on", "with", "he", "as", "you", "do", "at",
        "this", "but", "his", "by", "from", "they", "we", "say", "her", "she",
        "or", "an", "will", "my", "one", "all", "would", "there", "their", "what",
    ];
    for (i, w) in words.iter().enumerate() {
        trie1.insert(w.as_bytes().to_vec(), i as i32).unwrap();
    }
    eprintln!("STAK=1 arena size: {}", trie1.arena.len());

    // Verify STAK=1 lookups work
    for (i, w) in words.iter().enumerate() {
        assert!(trie1.get(w.as_bytes()).is_some(), "STAK=1 get({:?}) failed", w);
    }

    // Convert to STAK=2
    let mut trie2 = build_stak2_from_stak1(&trie1);
    eprintln!("STAK=2 arena size before optimize: {}", trie2.arena.len());

    // Verify STAK=2 lookups work BEFORE optimize
    for (i, w) in words.iter().enumerate() {
        let result = trie2.get(w.as_bytes());
        assert!(result.is_some(), "STAK=2 get({:?}) = None before optimize (expected {})", w, i + 1);
    }

    // Validate internal addresses before optimize
    for (phys, node) in trie2.arena.iter().enumerate() {
        let occ = node.occupancy[0];
        if occ == 0 && !node.is_terminal(0) { continue; }
        for nib in 0..16 {
            if (occ >> nib) & 1 == 0 { continue; }
            if node.is_leaf(nib, 0) { continue; }
            let addr = node.children[nib].as_usize();
            let child_phys = addr / 2;
            assert!(child_phys < trie2.arena.len(),
                "STAK=2 before optimize: phys={} nib={} has addr={} -> child_phys={} but arena len={}",
                phys, nib, addr, child_phys, trie2.arena.len());
        }
    }

    // Optimize
    trie2.optimize();
    eprintln!("STAK=2 arena size after optimize: {}", trie2.arena.len());

    // Verify STAK=2 lookups work AFTER optimize
    for (i, w) in words.iter().enumerate() {
        let result = trie2.get(w.as_bytes());
        assert!(result.is_some(), "STAK=2 get({:?}) = None after optimize (expected {})", w, i + 1);
    }

    // Validate internal addresses after optimize
    for (phys, node) in trie2.arena.iter().enumerate() {
        for v in 0..2 {
            let occ = node.occupancy[v];
            if occ == 0 && !node.is_terminal(v) { continue; }
            for nib in 0..16 {
                if (occ >> nib) & 1 == 0 { continue; }
                if node.is_leaf(nib, v) { continue; }
                let addr = node.children[nib].as_usize();
                if addr == u32::MAX as usize { continue; } // sentinel
                let child_phys = addr / 2;
                let child_vnode = addr % 2;
                assert!(child_phys < trie2.arena.len(),
                    "STAK=2 after optimize: phys={} v={} nib={} has addr={} -> child_phys={} child_vnode={} but arena len={}",
                    phys, v, nib, addr, child_phys, child_vnode, trie2.arena.len());
            }
        }
    }
}

#[test]
fn stak4_optimize_debug_diverse_keys() {
    let mut trie1: NibbleTrie<i32> = NibbleTrie::new();
    let words = [
        "the", "be", "to", "of", "and", "a", "in", "that", "have", "I",
        "it", "for", "not", "on", "with", "he", "as", "you", "do", "at",
        "this", "but", "his", "by", "from", "they", "we", "say", "her", "she",
        "or", "an", "will", "my", "one", "all", "would", "there", "their", "what",
    ];
    for (i, w) in words.iter().enumerate() {
        trie1.insert(w.as_bytes().to_vec(), i as i32).unwrap();
    }

    let mut trie4 = build_stak4_from_stak1(&trie1);
    eprintln!("STAK=4 arena size before optimize: {}", trie4.arena.len());

    // Verify STAK=4 lookups work BEFORE optimize
    for (i, w) in words.iter().enumerate() {
        let result = trie4.get(w.as_bytes());
        assert!(result.is_some(), "STAK=4 get({:?}) = None before optimize", w);
    }

    trie4.optimize();
    eprintln!("STAK=4 arena size after optimize: {}", trie4.arena.len());

    // Verify STAK=4 lookups work AFTER optimize
    for (i, w) in words.iter().enumerate() {
        let result = trie4.get(w.as_bytes());
        assert!(result.is_some(), "STAK=4 get({:?}) = None after optimize", w);
    }

    // Validate internal addresses after optimize
    for (phys, node) in trie4.arena.iter().enumerate() {
        for v in 0..4 {
            let occ = node.occupancy[v];
            if occ == 0 && !node.is_terminal(v) { continue; }
            for nib in 0..16 {
                if (occ >> nib) & 1 == 0 { continue; }
                if node.is_leaf(nib, v) { continue; }
                let addr = node.children[nib].as_usize();
                if addr == u32::MAX as usize { continue; }
                let child_phys = addr / 4;
                let child_vnode = addr % 4;
                assert!(child_phys < trie4.arena.len(),
                    "STAK=4 after optimize: phys={} v={} nib={} has addr={} -> child_phys={} child_vnode={} but arena len={}",
                    phys, v, nib, addr, child_phys, child_vnode, trie4.arena.len());
            }
        }
    }
}

/// Helper: build a STAK=4 trie from a STAK=1 trie.
fn build_stak4_from_stak1<T: Clone>(trie1: &NibbleTrie<T, u32, u16, 1>) -> NibbleTrie<T, u32, u16, 4> {
    let mut trie4: NibbleTrie<T, u32, u16, 4> = NibbleTrie::new();
    trie4.buf = trie1.buf.clone();
    trie4.index = trie1.index.clone();
    trie4.values = trie1.values.clone();
    for node1 in &trie1.arena {
        let mut node4: Node<u32, u16, 4> = Node::new();
        for nib in 0..16 {
            if node1.is_occupied(nib, 0) {
                if node1.is_leaf(nib, 0) {
                    node4.children[nib] = node1.children[nib];
                } else {
                    node4.children[nib] = u32::from_usize(node1.children[nib].as_usize() * 4);
                }
                node4.occupancy[0] |= 1 << nib;
                if node1.is_leaf(nib, 0) {
                    node4.leaf_mask[0] |= 1 << nib;
                }
            }
        }
        node4.prefix_len[0] = node1.prefix_len[0];
        node4.leaf = node1.leaf;
        node4.terminal = if node1.is_terminal(0) { 1 } else { 0 };
        trie4.arena.push(node4);
    }
    trie4
}

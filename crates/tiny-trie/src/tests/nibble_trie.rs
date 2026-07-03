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
    // "abc" is a prefix of "abcd" and sorts before it, so inserting it second
    // shifts "abcd"'s slot right — the index returned by the first insert is
    // NOT stable across later inserts. Check by value, not by captured index.
    trie.insert(b"abcd".to_vec(), 1).unwrap();
    trie.insert(b"abc".to_vec(), 2).unwrap();
    assert_eq!(trie.get_value(b"abcd"), Some(&1));
    assert_eq!(trie.get_value(b"abc"), Some(&2));
    assert_eq!(trie.len(), 2);
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
    let (off, len, _) = trie.index[root.leaf.get().as_usize()].as_ref().unwrap();
    let off = off.get();
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
    for i in 0..100 {
        let key = format!("key_{:03}", i);
        trie.insert(key.into_bytes(), i).unwrap();
    }
    trie.optimize();
    // optimize() re-spreads keys into the sparse 2*i+1 layout, so the index
    // returned by insert() is not stable across optimize; check the value
    // (the semantic lookup) instead.
    for i in 0..100 {
        let key = format!("key_{:03}", i);
        assert_eq!(trie.get_value(key.as_bytes()), Some(&i),
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
    for b in 1u8..=255 {
        trie.insert(vec![b], b as i32).unwrap();
    }
    trie.optimize();
    for b in 1u8..=255 {
        let key = vec![b];
        assert_eq!(trie.get_value(&key), Some(&(b as i32)),
            "lookup failed after optimize for byte {}", b);
    }
}

#[test]
fn optimize_stress_1000() {
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    for i in 0..1000u32 {
        let key = format!("key_{:05}", i);
        trie.insert(key.into_bytes(), i as i32).unwrap();
    }
    trie.optimize();
    for i in 0..1000u32 {
        let key = format!("key_{:05}", i);
        assert_eq!(trie.get_value(key.as_bytes()), Some(&(i as i32)),
            "lookup failed after optimize at i={}", i);
    }
}

#[test]
fn optimize_deeply_nested() {
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    let mut key = Vec::new();
    for i in 0..100 {
        key.push(b'a');
        trie.insert(key.clone(), i).unwrap();
    }
    trie.optimize();
    for i in 0..100 {
        let key = vec![b'a'; i + 1];
        assert_eq!(trie.get_value(&key), Some(&(i as i32)));
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
        let (off, len, _) = trie.index[ki].as_ref().unwrap();
        let off = off.get();
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

    // Verify that occupied index entries appear in sorted key order (the sparse
    // layout interleaves None gaps, so walk occupied slots in index order).
    let mut prev: Option<&[u8]> = None;
    for slot in trie.index.iter().skip(1) {
        let (off, len, _val) = match slot {
            Some(s) => s,
            None => continue,
        };
        let off = off.get();
        let key = &trie.buf[off..off + len.as_usize()];
        if let Some(p) = prev {
            assert!(p <= key,
                "index not sorted: {:?} > {:?}",
                std::str::from_utf8(p), std::str::from_utf8(key));
        }
        prev = Some(key);
    }

    // Verify values match their keys (value == last 5 digits of "key_NNNNN").
    let mut count = 0;
    for slot in trie.index.iter().skip(1) {
        let (off, len, val) = match slot {
            Some(s) => s,
            None => continue,
        };
        let off = off.get();
        let key = &trie.buf[off..off + len.as_usize()];
        let expected_val = std::str::from_utf8(key).unwrap()
            .strip_prefix("key_").unwrap().parse::<i32>().unwrap();
        assert_eq!(*val, expected_val,
            "value mismatch: got {}, expected {}", val, expected_val);
        count += 1;
    }
    assert_eq!(count, n);
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
    // "\0" and "\0\0" sort before "a\0b"/"a\0c", so the later inserts shift the
    // earlier slots. Indices are not stable across inserts — check by value.
    trie.insert(b"a\0b".to_vec(), 1).unwrap();
    trie.insert(b"a\0c".to_vec(), 2).unwrap();
    trie.insert(b"\0".to_vec(), 3).unwrap();
    trie.insert(b"\0\0".to_vec(), 4).unwrap();

    assert_eq!(trie.get_value(b"a\0b"), Some(&1));
    assert_eq!(trie.get_value(b"a\0c"), Some(&2));
    assert_eq!(trie.get_value(b"\0"), Some(&3));
    assert_eq!(trie.get_value(b"\0\0"), Some(&4));
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
    for i in 0..100 {
        let key = format!("key_{:03}", i);
        trie.insert(key.into_bytes(), i).unwrap();
    }
    trie.optimize();
    for i in 0..100 {
        let key = format!("key_{:03}", i);
        assert_eq!(trie.get_value(key.as_bytes()), Some(&i),
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
    // With u8 PTR, real key indices fit in 1..=255. The sparse 2n+1 optimize
    // layout and the 90% re-spread trigger fill the index well before 255 real
    // keys (optimize stops at n=127 since 2*128+1 overflows u8); once
    // index.len() reaches 255 the next insert is rejected.
    let mut count = 0u32;
    for i in 0..400u32 {
        let key = format!("k{:05}", i);
        match trie.insert(key.into_bytes(), i as i32) {
            Ok(_) => count += 1,
            Err(()) => break,
        }
    }
    // Must eventually overflow (u8 can't index beyond 255).
    assert!(count < 255, "u8 overflow never triggered: inserted {count}");
    assert!(count > 0);
    // After overflow, every inserted key still resolves to its value.
    for i in 0..count {
        let key = format!("k{:05}", i);
        assert_eq!(trie.get_value(key.as_bytes()), Some(&(i as i32)),
            "lookup failed for i={i} after u8 overflow");
    }
    assert_eq!(trie.len(), count as usize);
}

// ── promote/demote tests ─────────────────────────────────────────────

#[test]
fn promote_u8_to_u16() {
    let mut trie: NibbleTrie<Vec<u8>, i32, u8, u16> = NibbleTrie::new();
    for i in 0..50u32 {
        let key = format!("key_{:03}", i);
        trie.insert(key.into_bytes(), i as i32).unwrap();
    }
    let promoted: NibbleTrie<Vec<u8>, i32, u16, u16> = trie.promote::<u16>();
    // Indices are not stable across inserts/optimize — verify by value.
    for i in 0..50u32 {
        let key = format!("key_{:03}", i);
        assert_eq!(promoted.get_value(key.as_bytes()), Some(&(i as i32)),
            "lookup failed after promote for i={}", i);
    }
    assert_eq!(promoted.len(), 50);
}

#[test]
fn promote_u16_to_u32() {
    let mut trie: NibbleTrie<Vec<u8>, i32, u16, u16> = NibbleTrie::new();
    for i in 0..100u32 {
        let key = format!("key_{:03}", i);
        trie.insert(key.into_bytes(), i as i32).unwrap();
    }
    let promoted: NibbleTrie<Vec<u8>, i32, u32, u16> = trie.promote::<u32>();
    for i in 0..100u32 {
        let key = format!("key_{:03}", i);
        assert_eq!(promoted.get_value(key.as_bytes()), Some(&(i as i32)));
    }
}

#[test]
fn demote_u16_to_u8() {
    let mut trie: NibbleTrie<Vec<u8>, i32, u16, u16> = NibbleTrie::new();
    for i in 0..10u32 {
        let key = format!("key_{:03}", i);
        trie.insert(key.into_bytes(), i as i32).unwrap();
    }
    let demoted: NibbleTrie<Vec<u8>, i32, u8, u16> = match trie.demote::<u8>() {
        Ok(d) => d,
        Err(_) => panic!("demote should succeed with 10 keys"),
    };
    for i in 0..10u32 {
        let key = format!("key_{:03}", i);
        assert_eq!(demoted.get_value(key.as_bytes()), Some(&(i as i32)));
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

// ── Stage B invariant oracle + stress tests ──────────────────────────
//
// Insert indices are no longer stable across inserts (shift-based allocation
// moves earlier keys' slots). So these tests check the *structural invariants*
// directly — the actual correctness contract of the bump walk:
//
//   (1) every node's `leaf` == the min key index in its subtree (leftmost-leaf),
//   (2) occupied index slots are in non-decreasing key order (index sorted),
//   (3) every arena ref (node.leaf, leaf children) points to an occupied slot,
//   (4) n_keys == number of occupied slots,
// plus a BTreeMap cross-check of key/value presence and forward iteration order.

use std::collections::BTreeMap;

/// Deterministic xorshift64 PRNG (avoids a `rand` dev-dependency).
fn next_u64(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}

/// Random-ish key of length 1..=max_len from `state`.
fn rand_key(state: &mut u64, max_len: usize) -> Vec<u8> {
    let len = 1 + (next_u64(state) as usize % max_len);
    (0..len).map(|_| (next_u64(state) & 0xFF) as u8).collect()
}

/// Recompute the leftmost (min key index) of the subtree at `phys` and check
/// it equals `arena[phys].leaf`. Also checks every leaf-child / terminal ref in
/// the subtree points to an occupied index slot. Returns `Err(msg)` on violation.
fn recompute_leftmost<PTR: TrieIndex, LEN: TrieIndex>(
    trie: &NibbleTrie<Vec<u8>, i32, PTR, LEN>,
    phys: usize,
) -> Result<usize, String> {
    let node = &trie.arena[phys];
    let mut min_ki: Option<usize> = None;
    if node.is_terminal() {
        let ki = node.leaf.get().as_usize();
        let (_toff, tlen, _tval) = trie.index[ki].as_ref().ok_or_else(|| format!(
            "terminal node {phys}: leaf -> gap slot {ki}"))?;
        // A terminal key ends exactly at this node's depth, so its nibble length
        // must equal prefix_len. This catches a terminal node's `leaf` pointing
        // at a (longer) descendant instead of its own terminal key.
        if tlen.as_usize() * 2 != node.prefix_len.as_usize() {
            return Err(format!(
                "terminal node {phys}: leaf slot {ki} key len {} != prefix_len {} (not the terminal key)",
                tlen.as_usize() * 2, node.prefix_len.as_usize()));
        }
        min_ki = Some(ki);
    }
    for nib in 0..16 {
        if !node.is_occupied(nib) {
            continue;
        }
        let child_leftmost = if node.is_leaf(nib) {
            let ki = node.children[nib].get().as_usize();
            if !trie.index[ki].is_some() {
                return Err(format!(
                    "node {phys} leaf child nib {nib}: -> gap slot {ki}"));
            }
            ki
        } else {
            recompute_leftmost(trie, node.children[nib].get().as_usize())?
        };
        min_ki = Some(min_ki.map_or(child_leftmost, |m| m.min(child_leftmost)));
    }
    let l = min_ki.ok_or_else(|| format!("node {phys} has no keys in subtree"))?;
    if l != node.leaf.get().as_usize() {
        return Err(format!(
            "leftmost-leaf invariant violated at node {phys}: stored {}, recomputed {}",
            node.leaf.get().as_usize(), l));
    }
    Ok(l)
}

/// Full structural invariant check. Returns `Err(msg)` on the first violation.
fn verify_invariants<PTR: TrieIndex, LEN: TrieIndex>(
    trie: &NibbleTrie<Vec<u8>, i32, PTR, LEN>,
) -> Result<(), String> {
    if trie.arena.is_empty() {
        return if trie.n_keys == 0 { Ok(()) } else { Err("empty trie with nonzero n_keys".into()) };
    }
    recompute_leftmost(trie, 0)?;

    let mut prev: Option<Vec<u8>> = None;
    let mut occupied = 0usize;
    for (i, slot) in trie.index.iter().enumerate() {
        if let Some((off, len, _val)) = slot {
            let k = trie.buf[off.get()..off.get() + len.as_usize()].to_vec();
            if let Some(p) = &prev {
                if k < *p {
                    return Err(format!(
                        "index not sorted: slot {i} key {k:?} < prev {p:?}"));
                }
            }
            prev = Some(k);
            occupied += 1;
        }
    }
    if occupied != trie.n_keys {
        return Err(format!(
            "n_keys mismatch: {occupied} occupied slots vs n_keys {}", trie.n_keys));
    }
    Ok(())
}

/// Cross-check a trie against a BTreeMap oracle: every key resolves to its
/// value, no extra keys, and forward iteration matches the oracle's order.
fn cross_check_oracle<PTR: TrieIndex, LEN: TrieIndex>(
    trie: &NibbleTrie<Vec<u8>, i32, PTR, LEN>,
    oracle: &BTreeMap<Vec<u8>, i32>,
) {
    assert_eq!(trie.len(), oracle.len());
    for (k, v) in oracle {
        assert_eq!(trie.get_value(k), Some(v),
            "get_value mismatch for key {:?}", k);
    }
    // Forward iteration order == oracle order, with matching values.
    let mut it = trie.iter();
    let mut ordered = Vec::new();
    if let Some((k, v)) = it.current() {
        ordered.push((k.to_vec(), *v));
    }
    while let Some((k, v)) = it.next() {
        ordered.push((k.to_vec(), *v));
    }
    let expected: Vec<(Vec<u8>, i32)> =
        oracle.iter().map(|(k, v)| (k.clone(), *v)).collect();
    assert_eq!(ordered, expected, "forward iteration order/value mismatch");
}

fn stress_insert_sequence(keys: &[Vec<u8>]) {
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    let mut oracle: BTreeMap<Vec<u8>, i32> = BTreeMap::new();
    for (i, key) in keys.iter().enumerate() {
        // Skip duplicate keys (the trie rejects them); the oracle must too.
        if oracle.contains_key(key) {
            assert_eq!(trie.insert(key.clone(), i as i32), Err(()),
                "trie accepted duplicate key {:?}", key);
            continue;
        }
        trie.insert(key.clone(), i as i32).unwrap();
        oracle.insert(key.clone(), i as i32);
        // After EVERY insert, the structural invariants must hold — this is the
        // real test of the shift + bump walk.
        if let Err(msg) = verify_invariants(&trie) {
            panic!("after inserting key #{i} {:?}: {msg}", key);
        }
    }
    cross_check_oracle(&trie, &oracle);
}

#[test]
fn invariant_random_keys() {
    let mut state = 0x9e3779b97f4a7c15;
    let mut keys: Vec<Vec<u8>> = Vec::new();
    for _ in 0..500 {
        keys.push(rand_key(&mut state, 8));
    }
    stress_insert_sequence(&keys);
}

#[test]
fn invariant_sorted_keys() {
    // Distinct ascending keys: pure END-case appends + periodic optimize.
    let keys: Vec<Vec<u8>> = (0..500u32).map(|i| format!("key_{:05}", i).into_bytes()).collect();
    stress_insert_sequence(&keys);
}

#[test]
fn invariant_reverse_keys() {
    // Descending: every insert is the new smallest → max shifting + bumping.
    let keys: Vec<Vec<u8>> = (0..500u32)
        .rev()
        .map(|i| format!("key_{:05}", i).into_bytes())
        .collect();
    stress_insert_sequence(&keys);
}

#[test]
fn invariant_prefix_heavy() {
    // Many prefix relationships → Terminal + SplitNode + SplitLeaf cases.
    let mut keys: Vec<Vec<u8>> = Vec::new();
    for i in 0..200u32 {
        let base = format!("prefix_{:03}", i);
        keys.push(base.as_bytes().to_vec());
        keys.push(format!("{}{}", base, "_suffix").into_bytes());
        keys.push(format!("{}{}", base, "x").into_bytes());
    }
    stress_insert_sequence(&keys);
}

#[test]
fn invariant_mixed_lengths() {
    // Single-byte through long keys; exercises nibble boundary + prefix cases.
    let mut state = 0xdeadbeefcafebabe;
    let mut keys: Vec<Vec<u8>> = Vec::new();
    for _ in 0..400 {
        keys.push(rand_key(&mut state, 24));
    }
    stress_insert_sequence(&keys);
}

#[test]
fn invariant_backwards_iteration_after_shifts() {
    // Reverse insertion produces heavy shifting; verify reverse iteration too.
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    let mut oracle: BTreeMap<Vec<u8>, i32> = BTreeMap::new();
    for i in (0..300u32).rev() {
        let key = format!("k{:05}", i);
        let kb = key.into_bytes();
        trie.insert(kb.clone(), i as i32).unwrap();
        oracle.insert(kb, i as i32);
        if let Err(msg) = verify_invariants(&trie) { panic!("after inserting #{i}: {msg}"); }
    }
    assert_eq!(trie.len(), oracle.len());
    let mut it = trie.iter_last();
    let mut ordered = Vec::new();
    if let Some((k, v)) = it.current() {
        ordered.push((k.to_vec(), *v));
    }
    while let Some((k, v)) = it.prev() {
        ordered.push((k.to_vec(), *v));
    }
    let expected: Vec<(Vec<u8>, i32)> =
        oracle.iter().rev().map(|(k, v)| (k.clone(), *v)).collect();
    assert_eq!(ordered, expected, "reverse iteration mismatch after shifts");
}

#[test]
fn invariant_seek_after_shifts() {
    let mut state = 0x123456789abcdef0;
    let mut keys: Vec<Vec<u8>> = Vec::new();
    for _ in 0..300 {
        keys.push(rand_key(&mut state, 8));
    }
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    let mut oracle: BTreeMap<Vec<u8>, i32> = BTreeMap::new();
    for (i, key) in keys.iter().enumerate() {
        if oracle.contains_key(key) { continue; }
        trie.insert(key.clone(), i as i32).unwrap();
        oracle.insert(key.clone(), i as i32);
        if let Err(msg) = verify_invariants(&trie) {
            panic!("after inserting #{i} {:?}: {msg}", key);
        }
    }
    // Seek to each oracle key and to non-keys that fall between existing keys.
    for (k, v) in &oracle {
        let mut it = trie.iter();
        it.seek(k);
        assert_eq!(it.current(), Some((k.as_slice(), v)),
            "seek mismatch for key {:?}", k);
    }
    // Seek to a non-key lands on the first existing key >= it (the ceiling).
    // Construct a probe just past each existing key and compare to the oracle's
    // own ceiling (`range(probe..)`).
    for k in oracle.keys() {
        let mut probe = k.clone();
        *probe.last_mut().unwrap() = probe.last().copied().unwrap_or(0).wrapping_add(1);
        if oracle.contains_key(&probe) {
            continue;
        }
        let expected = oracle.keys().find(|k| k.as_slice() >= probe.as_slice()).cloned();
        let mut it = trie.iter();
        it.seek(&probe);
        assert_eq!(it.current().map(|(kk, _)| kk.to_vec()), expected,
            "seek-ceiling mismatch: seek {:?} expected {:?} got {:?}",
            probe, expected, it.current().map(|(kk, _)| kk.to_vec()));
    }
}

#[test]
fn invariant_past_optimize_repeatedly() {
    // Force many 90%-trigger re-spreads: dense distinct keys in a narrow space
    // so gaps deplete quickly and optimize fires repeatedly.
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    let mut oracle: BTreeMap<Vec<u8>, i32> = BTreeMap::new();
    for i in 0..1000u32 {
        let key = format!("{:04}", i); // narrow 4-char space, dense
        let kb = key.into_bytes();
        trie.insert(kb.clone(), i as i32).unwrap();
        oracle.insert(kb, i as i32);
        if let Err(msg) = verify_invariants(&trie) { panic!("after inserting #{i}: {msg}"); }
    }
    cross_check_oracle(&trie, &oracle);
}

// ---------------------------------------------------------------------------
// Cursor — linear-scan iterator over the sparse index
// ---------------------------------------------------------------------------

#[test]
fn cursor_empty_trie() {
    let trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    let mut it = trie.iter();
    assert!(it.current().is_none());
    assert!(it.current_index().is_none());
    assert!(it.next().is_none());
    assert!(it.prev().is_none());
    assert!(it.first().is_none());
    assert!(it.last().is_none());
    assert!(it.seek(b"anything").is_none());
}

#[test]
fn cursor_forward_before_first() {
    // iter() parks *before* the first key: current() is None, next() yields first.
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"abd".to_vec(), 2).unwrap();
    trie.insert(b"abe".to_vec(), 3).unwrap();

    let mut it = trie.iter();
    assert!(it.current().is_none());
    assert_eq!(it.next().unwrap().0, b"abc");
    assert_eq!(it.current().unwrap().0, b"abc"); // cached — same value, no re-scan
    assert_eq!(it.next().unwrap().0, b"abd");
    assert_eq!(it.next().unwrap().0, b"abe");
    assert!(it.next().is_none());
    // Forward-exhausted: current() stays None, but prev() walks back.
    assert!(it.current().is_none());
    assert_eq!(it.prev().unwrap().0, b"abe");
}

#[test]
fn cursor_backward_on_last() {
    // iter_last() parks *on* the last key.
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"abd".to_vec(), 2).unwrap();
    trie.insert(b"abe".to_vec(), 3).unwrap();

    let mut it = trie.iter_last();
    assert_eq!(it.current().unwrap().0, b"abe");
    assert!(it.current_index().is_some()); // parked on a real slot
    assert_eq!(it.prev().unwrap().0, b"abd");
    assert_eq!(it.prev().unwrap().0, b"abc");
    assert!(it.prev().is_none());
    // Before-first: next() walks forward again.
    assert_eq!(it.next().unwrap().0, b"abc");
}

#[test]
fn cursor_first_last_jump() {
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    trie.insert(b"mid".to_vec(), 5).unwrap();
    trie.insert(b"aaa".to_vec(), 1).unwrap();
    trie.insert(b"zzz".to_vec(), 9).unwrap();

    let mut it = trie.iter();
    assert_eq!(it.first().unwrap().0, b"aaa");
    assert_eq!(it.current().unwrap().0, b"aaa");
    assert_eq!(it.last().unwrap().0, b"zzz");
    assert_eq!(it.current().unwrap().0, b"zzz");
    // From the last key, prev walks backward; from first, next walks forward.
    assert_eq!(it.prev().unwrap().0, b"mid");
    assert_eq!(it.first().unwrap().0, b"aaa");
    assert_eq!(it.next().unwrap().0, b"mid");
}

#[test]
fn cursor_seek_then_scan() {
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    for i in 0..10u32 {
        trie.insert(format!("key{i}").into_bytes(), i as i32).unwrap();
    }
    let mut it = trie.iter();
    // seek lands on the first key >= "key5" (exact match here).
    assert_eq!(it.seek(b"key5").unwrap().0, b"key5");
    assert_eq!(it.next().unwrap().0, b"key6");
    assert_eq!(it.prev().unwrap().0, b"key5"); // back to where seek landed
    assert_eq!(it.prev().unwrap().0, b"key4");
    // Seek past the end → exhausted.
    assert!(it.seek(b"zzz").is_none());
    assert!(it.current().is_none());
    // Seek before the beginning → lands on first.
    assert_eq!(it.seek(b"").unwrap().0, b"key0");
}

#[test]
fn cursor_scan_order_matches_oracle_after_shifts() {
    // Shift-heavy insert sequence (reverse order forces many non-END shifts),
    // then verify the linear-scan cursor matches BTreeMap order both forward
    // and backward, including after an optimize re-spread.
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    let mut oracle: BTreeMap<Vec<u8>, i32> = BTreeMap::new();
    for i in (0..200u32).rev() {
        let k = format!("item{:04}", i).into_bytes();
        trie.insert(k.clone(), i as i32).unwrap();
        oracle.insert(k, i as i32);
    }
    trie.optimize();

    let mut fwd: Vec<(Vec<u8>, i32)> = Vec::new();
    let mut it = trie.iter();
    if let Some((k, v)) = it.current() { fwd.push((k.to_vec(), *v)); }
    while let Some((k, v)) = it.next() { fwd.push((k.to_vec(), *v)); }
    let expected: Vec<(Vec<u8>, i32)> =
        oracle.iter().map(|(k, v)| (k.clone(), *v)).collect();
    assert_eq!(fwd, expected, "forward scan order mismatch after shifts+optimize");

    let mut rev: Vec<(Vec<u8>, i32)> = Vec::new();
    let mut it = trie.iter_last();
    if let Some((k, v)) = it.current() { rev.push((k.to_vec(), *v)); }
    while let Some((k, v)) = it.prev() { rev.push((k.to_vec(), *v)); }
    let expected_rev: Vec<(Vec<u8>, i32)> =
        oracle.iter().rev().map(|(k, v)| (k.clone(), *v)).collect();
    assert_eq!(rev, expected_rev, "backward scan order mismatch after shifts+optimize");
}

// ---------------------------------------------------------------------------
// THROWAWAY: print a memory breakdown of a random-filled trie (before & after
// optimize). Run with: cargo test -p tiny-trie mem_print -- --nocapture --ignored
// ---------------------------------------------------------------------------

#[test]
#[ignore]
fn mem_print() {
    use std::mem::{size_of, size_of_val};

    type Trie = NibbleTrie<Vec<u8>, i32>; // PTR=u32, LEN=u16

    let mut trie: Trie = NibbleTrie::new();
    let mut state: u64 = 0x9e3779b97f4a7c15;
    const N: usize = 50_000;
    const MAX_LEN: usize = 16;

    let mut raw_key_bytes: usize = 0;
    let mut inserted = 0usize;
    while inserted < N {
        let k = rand_key(&mut state, MAX_LEN);
        // Skip duplicates so inserted == distinct key count.
        if trie.get(&k).is_some() {
            continue;
        }
        trie.insert(k.clone(), inserted as i32).unwrap();
        raw_key_bytes += k.len();
        inserted += 1;
    }

    let val_bytes = trie.len() * size_of::<i32>();
    // Node size via the arena element type; slot size via a live Some slot.
    let node_size = size_of_val(&trie.arena[0]);
    let slot_size = size_of_val(&trie.index[1]); // index[1] is a Some slot after first insert

    let report = |label: &str, t: &Trie| {
        let n = t.len();
        let idx_len = t.index.len();
        let idx_cap = t.index.capacity();
        let arena_cap = t.arena.capacity();
        let buf_cap = t.buf.capacity();
        let arena_bytes = arena_cap * node_size;
        let buf_bytes = buf_cap;
        let index_bytes = idx_cap * slot_size;
        let total = arena_bytes + buf_bytes + index_bytes;
        let gaps = idx_len - n; // None slots in the live index
        let mut s = String::new();
        s.push_str(&format!("\n=== {label} ===\n"));
        s.push_str(&format!("  n_keys (distinct)      : {n}\n"));
        s.push_str(&format!("  raw key bytes          : {raw_key_bytes}  ({:.1} B/key avg)\n", raw_key_bytes as f64 / n as f64));
        s.push_str(&format!("  raw value bytes        : {val_bytes}  ({} B/val)\n", size_of::<i32>()));
        s.push_str(&format!("  raw keys+values        : {}\n", raw_key_bytes + val_bytes));
        s.push_str(&format!("  sizes: Node={node_size} B, Option<Slot>={slot_size} B\n"));
        s.push_str(&format!("  arena  : len={} cap={} -> {} B\n", t.arena.len(), arena_cap, arena_bytes));
        s.push_str(&format!("  buf    : len={} cap={} -> {} B\n", t.buf.len(), buf_cap, buf_bytes));
        s.push_str(&format!("  index  : len={} cap={} -> {} B  (occupied={n}, gaps={gaps})\n", idx_len, idx_cap, index_bytes));
        s.push_str(&format!("  TOTAL reserved         : {} B  ({:.2} MiB)\n", total, total as f64 / (1 << 20) as f64));
        s.push_str(&format!("  overhead vs raw        : {:.1}x  ({} B raw -> {} B reserved)\n",
                 total as f64 / (raw_key_bytes + val_bytes) as f64,
                 raw_key_bytes + val_bytes, total));
        print!("{s}");
        // Also append to a file so token-filtering proxies don't swallow it.
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true).append(true)
            .open("/tmp/nibble_mem.txt").expect("open /tmp/nibble_mem.txt");
        f.write_all(s.as_bytes()).expect("write");
    };

    report("BEFORE optimize (post-insert, with 90%-trigger re-spreads)", &trie);
    trie.optimize();
    report("AFTER  optimize (explicit single optimize)", &trie);

    // Touch results so the compiler doesn't elide anything.
    std::hint::black_box(&trie);
}

// ---------------------------------------------------------------------------
// THROWAWAY: print node-occupancy stats for a random-filled trie. Run with:
// cargo test -p tiny-trie node_stats -- --nocapture --ignored
// ---------------------------------------------------------------------------

#[test]
#[ignore]
fn node_stats() {
    type Trie = NibbleTrie<Vec<u8>, i32>; // PTR=u32, LEN=u16

    let mut trie: Trie = NibbleTrie::new();
    let mut state: u64 = 0x9e3779b97f4a7c15;
    const N: usize = 50_000;
    const MAX_LEN: usize = 16;

    let mut inserted = 0usize;
    while inserted < N {
        let k = rand_key(&mut state, MAX_LEN);
        if trie.get(&k).is_some() {
            continue;
        }
        trie.insert(k, inserted as i32).unwrap();
        inserted += 1;
    }
    // optimize() doesn't change arena topology, so node stats are the same
    // before/after it — report once, on the post-insert tree.
    trie.optimize();

    // occupancy = occupied child slots + (1 if terminal). Range 0..=17.
    let mut hist_incl_term: [usize; 18] = [0; 18];
    // occupied child slots only (0..=16), terminal counted separately.
    let mut hist_children: [usize; 17] = [0; 17];
    let mut terminal_nodes = 0usize;
    let mut total_leaf_edges = 0usize;   // children[nib] that are leaf key indices
    let mut total_internal_edges = 0usize; // children[nib] that are arena indices
    let mut total_children = 0usize;

    for node in trie.arena.iter() {
        let mask = node.children_mask();
        let occ = mask.count_ones() as usize;
        let term = node.is_terminal();
        hist_children[occ] += 1;
        let occ_incl = occ + term as usize;
        hist_incl_term[occ_incl] += 1;
        if term { terminal_nodes += 1; }
        total_children += occ;
        // Leaf vs internal edges: leaf_mask bit set => leaf child.
        total_leaf_edges += (node.leaf_mask & mask).count_ones() as usize;
        // The rest of the occupied slots are internal (arena) children.
        total_internal_edges += occ - (node.leaf_mask & mask).count_ones() as usize;
    }

    let total_nodes = trie.arena.len();
    let avg_children = total_children as f64 / total_nodes as f64;

    let mut s = String::new();
    s.push_str(&format!("\n=== node stats (N={N} keys, max_len={MAX_LEN}) ===\n"));
    s.push_str(&format!("  total nodes            : {total_nodes}\n"));
    s.push_str(&format!("  terminal nodes         : {terminal_nodes}  ({:.1}%)\n",
        100.0 * terminal_nodes as f64 / total_nodes as f64));
    s.push_str(&format!("  total child edges      : {total_children}  (avg {avg_children:.2}/node)\n"));
    s.push_str(&format!("    leaf edges (->key)   : {total_leaf_edges}\n"));
    s.push_str(&format!("    internal edges(->node): {total_internal_edges}\n"));
    s.push_str(&format!("  nodes/key              : {:.3}\n", total_nodes as f64 / N as f64));

    s.push_str("\n  occupancy histogram (occupied child slots + terminal flag):\n");
    s.push_str("    occ |  nodes   %\n");
    s.push_str("    ----+----------------\n");
    for (occ, &cnt) in hist_incl_term.iter().enumerate() {
        if cnt == 0 { continue; }
        s.push_str(&format!("    {occ:>3} | {cnt:>6}  {:>5.1}%\n",
            100.0 * cnt as f64 / total_nodes as f64));
    }

    s.push_str("\n  child-count histogram (occupied child slots only):\n");
    s.push_str("    kids |  nodes   %\n");
    s.push_str("    -----+----------------\n");
    for (kids, &cnt) in hist_children.iter().enumerate() {
        if cnt == 0 { continue; }
        s.push_str(&format!("    {kids:>4} | {cnt:>6}  {:>5.1}%\n",
            100.0 * cnt as f64 / total_nodes as f64));
    }
    print!("{s}");
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true).append(true)
        .open("/tmp/nibble_nodes.txt").expect("open /tmp/nibble_nodes.txt");
    f.write_all(s.as_bytes()).expect("write");
    std::hint::black_box(&trie);
}

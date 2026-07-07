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
    trie.insert(b"hello".to_vec(), 42).unwrap();
    assert_eq!(trie.get(b"hello"), Some(&42));
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
    trie.insert(b"hel\0lo".to_vec(), 1).unwrap();
    assert_eq!(trie.get(b"hel\0lo"), Some(&1));
}

#[test]
fn insert_two_keys_split_leaf() {
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"abd".to_vec(), 2).unwrap();
    assert_eq!(trie.get(b"abc"), Some(&1));
    assert_eq!(trie.get(b"abd"), Some(&2));
    assert_eq!(trie.len(), 2);
}

#[test]
fn insert_prefix_key() {
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"abcd".to_vec(), 2).unwrap();
    assert_eq!(trie.get(b"abc"), Some(&1));
    assert_eq!(trie.get(b"abcd"), Some(&2));
}

#[test]
fn insert_reverse_prefix_key() {
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    // "abc" is a prefix of "abcd" and sorts before it, so inserting it second
    // shifts "abcd"'s slot right — the index returned by the first insert is
    // NOT stable across later inserts. Check by value, not by captured index.
    trie.insert(b"abcd".to_vec(), 1).unwrap();
    trie.insert(b"abc".to_vec(), 2).unwrap();
    assert_eq!(trie.get(b"abcd"), Some(&1));
    assert_eq!(trie.get(b"abc"), Some(&2));
    assert_eq!(trie.len(), 2);
}

#[test]
fn insert_no_common_prefix() {
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"xyz".to_vec(), 2).unwrap();
    assert_eq!(trie.get(b"abc"), Some(&1));
    assert_eq!(trie.get(b"xyz"), Some(&2));
}

#[test]
fn insert_three_keys() {
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"abd".to_vec(), 2).unwrap();
    trie.insert(b"abe".to_vec(), 3).unwrap();
    assert_eq!(trie.get(b"abc"), Some(&1));
    assert_eq!(trie.get(b"abd"), Some(&2));
    assert_eq!(trie.get(b"abe"), Some(&3));
}

#[test]
fn insert_single_char_keys() {
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    for c in b'a'..=b'f' {
        trie.insert(vec![c], c as i32).unwrap();
    }
    for c in b'a'..=b'f' {
        let key = vec![c];
        let v = c as i32;
        assert_eq!(trie.get(&key), Some(&v));
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
        trie.insert(key.clone(), i).unwrap();
        assert_eq!(trie.get(&key), Some(&i));
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
fn get_found_and_missing() {
    let mut trie: NibbleTrie<Vec<u8>, String> = NibbleTrie::new();
    trie.insert(b"hello".to_vec(), "world".to_string()).unwrap();
    assert_eq!(trie.get(b"hello"), Some(&"world".to_string()));
    assert_eq!(trie.get(b"world"), None);
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
    let root = trie.inode(0);
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
    trie.insert(b"hello".to_vec(), 42).unwrap();
    trie.optimize();
    assert_eq!(trie.get(b"hello"), Some(&42));
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
        assert_eq!(trie.get(key.as_bytes()), Some(&i),
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
        assert_eq!(trie.get(&key), Some(&(b as i32)),
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
        assert_eq!(trie.get(key.as_bytes()), Some(&(i as i32)),
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
        assert_eq!(trie.get(&key), Some(&(i as i32)));
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
    assert_eq!(trie.get(b"ab"), Some(&1), "terminal key 'ab' lost after optimize");
    assert_eq!(trie.get(b"abcd"), Some(&2));
    assert_eq!(trie.len(), 2);

    // Also test reverse order
    let mut trie2: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    trie2.insert(b"abcd".to_vec(), 1).unwrap();
    trie2.insert(b"ab".to_vec(), 2).unwrap();
    trie2.optimize();
    assert_eq!(trie2.get(b"abcd"), Some(&1));
    assert_eq!(trie2.get(b"ab"), Some(&2), "terminal key 'ab' lost after optimize (reverse insert)");
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

    assert_eq!(trie.get(b"a\0b"), Some(&1));
    assert_eq!(trie.get(b"a\0c"), Some(&2));
    assert_eq!(trie.get(b"\0"), Some(&3));
    assert_eq!(trie.get(b"\0\0"), Some(&4));
    assert_eq!(trie.len(), 4);
}

// ── Compact mode tests (u16/u16) ──────────────────────────────────

#[test]
fn compact_insert_and_get() {
    let mut trie: NibbleTrie<Vec<u8>, i32, u16, u16> = NibbleTrie::new();
    trie.insert(b"hello".to_vec(), 42).unwrap();
    assert_eq!(trie.get(b"hello"), Some(&42));
    assert_eq!(trie.get(b"world"), None);
}

#[test]
fn compact_insert_prefix_keys() {
    let mut trie: NibbleTrie<Vec<u8>, i32, u16, u16> = NibbleTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"abcd".to_vec(), 2).unwrap();
    assert_eq!(trie.get(b"abc"), Some(&1));
    assert_eq!(trie.get(b"abcd"), Some(&2));
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

// ── CursorMut tests ──────────────────────────────────────────────

#[test]
fn iter_mut_forward_updates_values() {
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"abd".to_vec(), 2).unwrap();
    trie.insert(b"abe".to_vec(), 3).unwrap();

    // Increment every value in forward order; the &mut T borrows the cursor,
    // so each must be released before the next next() call.
    {
        let mut c = trie.iter_mut();
        while let Some((_, v)) = c.next() {
            *v += 100;
        }
    }
    assert_eq!(trie.get(b"abc"), Some(&101));
    assert_eq!(trie.get(b"abd"), Some(&102));
    assert_eq!(trie.get(b"abe"), Some(&103));
}

#[test]
fn iter_mut_backward_updates_values() {
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"abd".to_vec(), 2).unwrap();
    trie.insert(b"abe".to_vec(), 3).unwrap();

    {
        let mut c = trie.iter_mut_last();
        // current() reborrows each call; safe to call repeatedly.
        if let Some((_, v)) = c.current() { *v *= 10; }
        while let Some((_, v)) = c.prev() { *v *= 10; }
    }
    assert_eq!(trie.get(b"abc"), Some(&10));
    assert_eq!(trie.get(b"abd"), Some(&20));
    assert_eq!(trie.get(b"abe"), Some(&30));
}

#[test]
fn iter_mut_seek_then_mutate() {
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    for i in 0..50u32 {
        let key = format!("key_{:05}", i);
        trie.insert(key.into_bytes(), i as i32).unwrap();
    }
    {
        let mut c = trie.iter_mut();
        let (k, v) = c.seek(b"key_00025").unwrap();
        assert_eq!(k, b"key_00025");
        assert_eq!(*v, 25);
        *v = -1;
    }
    assert_eq!(trie.get(b"key_00025"), Some(&-1));
    // Other values untouched.
    assert_eq!(trie.get(b"key_00024"), Some(&24));
    assert_eq!(trie.get(b"key_00026"), Some(&26));
}

#[test]
fn iter_mut_empty() {
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    let mut c = trie.iter_mut();
    assert!(c.next().is_none());
    assert!(c.current().is_none());
}

#[test]
fn iter_mut_current_revisits_same_slot() {
    // The soundness-critical case from the design discussion: calling
    // current() twice must return the same value, with the first borrow
    // released before the second. This compiles only because &mut T is tied
    // to &mut self (lending), not to the trie lifetime.
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    trie.insert(b"k".to_vec(), 7).unwrap();
    let mut c = trie.iter_mut();
    c.first();
    {
        let (_, v) = c.current().unwrap();
        assert_eq!(*v, 7);
    } // borrow released here
    {
        let (_, v) = c.current().unwrap();
        *v = 9;
    }
    assert_eq!(trie.get(b"k"), Some(&9));
}

#[test]
fn iter_mut_yields_borrowed_key() {
    // CursorMut yields ByteKey::Borrowed<'_> — for Vec<u8> keys that's &mut T
    // alongside a borrowed &[u8] key, no allocation.
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    trie.insert(b"hello".to_vec(), 1).unwrap();
    let mut c = trie.iter_mut();
    let (k, _): (&[u8], &mut i32) = c.next().unwrap();
    assert_eq!(k, b"hello");
}

#[test]
fn cursor_yields_borrowed_str_for_string_keys() {
    // The payoff of ByteKey::Borrowed: a String-keyed trie yields &str, not
    // &[u8] and not an allocated String. Zero allocation per element.
    let mut trie: NibbleTrie<String, i32> = NibbleTrie::new();
    trie.insert("hello".to_string(), 1).unwrap();
    trie.insert("world".to_string(), 2).unwrap();

    let mut c = trie.iter();
    let (k, v): (&str, &i32) = c.next().unwrap();
    assert_eq!(k, "hello");
    assert_eq!(*v, 1);
    let (k, v): (&str, &i32) = c.next().unwrap();
    assert_eq!(k, "world");
    assert_eq!(*v, 2);
}

#[test]
fn cursor_mut_yields_borrowed_str_for_string_keys() {
    // CursorMut on a String-keyed trie yields (&str, &mut T).
    let mut trie: NibbleTrie<String, i32> = NibbleTrie::new();
    trie.insert("abc".to_string(), 1).unwrap();
    trie.insert("abd".to_string(), 2).unwrap();
    {
        let mut c = trie.iter_mut();
        while let Some((k, v)) = c.next() {
            // k is &str here, not &[u8] and not String.
            let _: &str = k;
            *v += 100;
        }
    }
    assert_eq!(trie.get(b"abc"), Some(&101));
    assert_eq!(trie.get(b"abd"), Some(&102));
}

#[test]
fn cursor_yields_borrowed_for_vec_keys() {
    // Immutable cursor yields ByteKey::Borrowed<'a> — for Vec<u8> that's &[u8].
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    trie.insert(b"hello".to_vec(), 1).unwrap();
    let mut c = trie.iter();
    let (k, _): (&[u8], &i32) = c.next().unwrap();
    assert_eq!(k, b"hello");
}

#[test]
fn iter_mut_compact_mode() {
    // CursorMut works under the u16/u16 compact encoding too.
    let mut trie: NibbleTrie<Vec<u8>, i32, u16, u16> = NibbleTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"abd".to_vec(), 2).unwrap();
    {
        let mut c = trie.iter_mut();
        while let Some((_, v)) = c.next() { *v += 1; }
    }
    assert_eq!(trie.get(b"abc"), Some(&2));
    assert_eq!(trie.get(b"abd"), Some(&3));
}

#[test]
fn iter_mut_first_last() {
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"abd".to_vec(), 2).unwrap();
    trie.insert(b"abe".to_vec(), 3).unwrap();
    let mut c = trie.iter_mut();
    let (k, v) = c.first().unwrap();
    assert_eq!(k, b"abc");
    assert_eq!(*v, 1);
    let (k, v) = c.last().unwrap();
    assert_eq!(k, b"abe");
    assert_eq!(*v, 3);
}

#[test]
fn iter_mut_index_tracking() {
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"abd".to_vec(), 2).unwrap();
    let mut c = trie.iter_mut();
    let i0 = c.next_index().unwrap();
    let i1 = c.next_index().unwrap();
    assert!(c.next_index().is_none());
    // The two occupied slots are distinct indices.
    assert_ne!(i0, i1);
    assert!(i0 > 0 && i1 > 0);
}

// ── Range iterator tests ─────────────────────────────────────────

fn range_trie() -> NibbleTrie<Vec<u8>, i32> {
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    for s in ["abc", "abd", "abe", "abf", "acd", "ace"] {
        trie.insert(s.as_bytes().to_vec(), s.len() as i32).unwrap();
    }
    trie
}

#[test]
fn range_inclusive_exclusive_bounds() {
    let trie = range_trie();
    // b"abd"..b"ace" → [abd, abe, abf, acd]  (abd included, ace excluded)
    let got: Vec<&[u8]> = trie.range(b"abd".as_slice()..b"ace".as_slice())
        .map(|(k, _)| k).collect();
    assert_eq!(got, vec![b"abd", b"abe", b"abf", b"acd"].as_slice());
}

#[test]
fn range_open_lower() {
    let trie = range_trie();
    // ..b"abe" → [abc, abd]
    let got: Vec<&[u8]> = trie.range(..b"abe".as_slice()).map(|(k, _)| k).collect();
    assert_eq!(got, vec![b"abc", b"abd"]);
}

#[test]
fn range_open_upper() {
    let trie = range_trie();
    // b"abe".. → [abe, abf, acd, ace]
    let got: Vec<&[u8]> = trie.range(b"abe".as_slice()..).map(|(k, _)| k).collect();
    assert_eq!(got, vec![b"abe", b"abf", b"acd", b"ace"]);
}

#[test]
fn range_full() {
    let trie = range_trie();
    let got: Vec<&[u8]> = trie.range(..).map(|(k, _)| k).collect();
    assert_eq!(got, vec![b"abc", b"abd", b"abe", b"abf", b"acd", b"ace"]);
}

#[test]
fn range_bound_included_excluded() {
    use std::ops::Bound;
    let trie = range_trie();
    // (Included(abe), Included(acd)) → [abe, abf, acd]
    let got: Vec<&[u8]> = trie
        .range_bounds(Bound::Included(b"abe".as_slice()), Bound::Included(b"acd".as_slice()))
        .map(|(k, _)| k).collect();
    assert_eq!(got, vec![b"abe", b"abf", b"acd"]);
    // (Excluded(abc), Excluded(ace)) → [abd, abe, abf, acd]
    let got: Vec<&[u8]> = trie
        .range_bounds(Bound::Excluded(b"abc".as_slice()), Bound::Excluded(b"ace".as_slice()))
        .map(|(k, _)| k).collect();
    assert_eq!(got, vec![b"abd", b"abe", b"abf", b"acd"]);
}

#[test]
fn range_empty_and_misses() {
    let trie = range_trie();
    // Empty: lower beyond every key.
    assert_eq!(trie.range(b"az".as_slice()..).count(), 0);
    // Empty: lower > upper.
    assert_eq!(trie.range(b"ace".as_slice()..b"abc".as_slice()).count(), 0);
    // Lower bound between keys lands on the ceiling (no phantom element).
    let got: Vec<&[u8]> = trie.range(b"abx".as_slice()..b"ace".as_slice()).map(|(k, _)| k).collect();
    assert_eq!(got, vec![b"acd"]); // ceiling of "abx" is "acd"; upper "ace" excluded
}

#[test]
fn range_double_ended() {
    let trie = range_trie();
    // Interleave next and next_back from b"abd"..b"ace":
    //   next → abd (smallest remaining), next_back → acd (largest remaining),
    //   next → abe, next_back → abf  →  [abd, acd, abe, abf]
    let mut it = trie.range(b"abd".as_slice()..b"ace".as_slice());
    let mut got = Vec::new();
    got.push(it.next().unwrap().0);
    got.push(it.next_back().unwrap().0);
    got.push(it.next().unwrap().0);
    got.push(it.next_back().unwrap().0);
    assert!(it.next().is_none() && it.next_back().is_none());
    assert_eq!(got, vec![b"abd", b"acd", b"abe", b"abf"]);
}

#[test]
fn range_values_correct() {
    let trie = range_trie();
    // Values are the key lengths (3 for all these single-segment keys, but the
    // point is the value is yielded alongside, not just keys).
    let pairs: Vec<(&[u8], i32)> = trie.range(b"abd".as_slice()..b"abf".as_slice())
        .map(|(k, v)| (k, *v)).collect();
    assert_eq!(pairs, vec![(b"abd".as_slice(), 3), (b"abe".as_slice(), 3)]);
}

#[test]
fn range_yields_borrowed_str_for_string_keys() {
    let mut trie: NibbleTrie<String, i32> = NibbleTrie::new();
    for s in ["apple", "banana", "cherry", "date"] {
        trie.insert(s.to_string(), s.len() as i32).unwrap();
    }
    // "banana"..="cherry" → [banana, cherry], yielded as &str, no allocation.
    let got: Vec<&str> = trie
        .range((Bound::Included(b"banana".as_slice()), Bound::Included(b"cherry".as_slice())))
        .map(|(k, _)| k).collect();
    assert_eq!(got, vec!["banana", "cherry"]);
}

#[test]
fn range_after_optimize() {
    // optimize() rebuilds the arena/index; range must still be correct.
    let mut trie = range_trie();
    trie.optimize();
    let got: Vec<&[u8]> = trie.range(b"abd".as_slice()..b"acd".as_slice()).map(|(k, _)| k).collect();
    assert_eq!(got, vec![b"abd", b"abe", b"abf"]);
}

#[test]
fn range_compact_mode() {
    let mut trie: NibbleTrie<Vec<u8>, i32, u16, u16> = NibbleTrie::new();
    for s in ["abc", "abd", "abe", "abf"] {
        trie.insert(s.as_bytes().to_vec(), s.len() as i32).unwrap();
    }
    let got: Vec<&[u8]> = trie.range(b"abd".as_slice()..b"abf".as_slice()).map(|(k, _)| k).collect();
    assert_eq!(got, vec![b"abd", b"abe"]);
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
        assert_eq!(trie.get(key.as_bytes()), Some(&i),
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
    trie.insert(b"hello".to_vec(), 42).unwrap();
    assert_eq!(trie.get(b"hello"), Some(&42));
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
        assert_eq!(trie.get(key.as_bytes()), Some(&(i as i32)),
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
        assert_eq!(promoted.get(key.as_bytes()), Some(&(i as i32)),
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
        assert_eq!(promoted.get(key.as_bytes()), Some(&(i as i32)));
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
        assert_eq!(demoted.get(key.as_bytes()), Some(&(i as i32)));
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
    assert_eq!(trie.get(b"hello"), Some(&1));
    assert_eq!(trie.get(b"world"), Some(&2));
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
    // An Fnode's leftmost = `base` (the smallest key index in its subtree). Verify
    // `base` and every terminal slot's key index points at an occupied slot.
    if let ArenaNode::Fnode(f) = &trie.arena[phys] {
        let base = f.base.as_usize();
        if trie.index[base].is_none() {
            return Err(format!("Fnode {phys}: base -> gap slot {base}"));
        }
        for (_plen, offset) in f.slots.as_slice() {
            if *offset != FNODE_OFFSET_NULL {
                let ki = base + *offset as usize;
                if trie.index[ki].is_none() {
                    return Err(format!("Fnode {phys}: slot offset {offset} -> gap slot {ki}"));
                }
            }
        }
        return Ok(base);
    }
    let node = trie.inode(phys);
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
        assert_eq!(trie.get(k), Some(v),
            "get mismatch for key {:?}", k);
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
    for (k, _v) in &oracle {
        let mut it = trie.iter();
        it.seek(k);
        assert_eq!(it.current().map(|(kk, _)| kk.to_vec()), Some(k.clone()),
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
        // Step 2: every arena element is an Inode (no Fnodes produced yet).
        // Match the Inode out; Fnode stats land in a later step.
        let node = match node {
            ArenaNode::Inode(n) => n,
            ArenaNode::Fnode(_) => continue,
        };
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

// ── Fnode (FlatNode) read-path tests ──────────────────────────────────
//
// Step 4 (revised encoding): read-only Fnodes use `base` + `terminal` +
// relative `u8` offsets (CAP 16: 1 `base` + 15 array slots). No code path
// *produces* Fnodes yet (flatten() is step 4's remaining work), so these tests
// build them by hand: insert keys via the normal Inode path, then *collapse* a
// chosen non-root Inode subtree into a `FlatNode` and verify `get`/
// `get_unchecked`/`seek`/forward-iter still match a `BTreeMap` oracle. The
// step-3 "subtree root can't be terminal" restriction is LIFTED — the root's
// own terminal key is pulled out of the array into `base`+`terminal: true` and
// returned by `flat_get`'s fallback path.

/// Flatten the Inode subtree rooted at `phys` into a [`FlatNode`] using the
/// pre-order DFS layout consumed by [`NibbleTrie::flat_get`].
///
/// Encoding (step 4, revised):
/// - `base` = the leftmost key's `index` position = `root.leaf` (the root's own
///   key when the root is terminal, else its leftmost descendant). It is the
///   smallest key in the subtree, hence the smallest `index` position, so every
///   other subtree key yields a non-negative offset.
/// - `terminal` = `root.is_terminal()`. When `true`, `base` is the root's own
///   prefix key: it is pulled OUT of the array (not a slot) and returned by
///   `flat_get`'s fallback. When `false`, `base` is a leftmost descendant and IS
///   emitted as an array slot (offset 0) — the fallback never returns it.
/// - Each array slot is `(prefix_len, offset)`: `prefix_len` = the discriminant
///   depth (the parent node's `prefix_len`, where this key diverges from a
///   sibling); `offset = key_index - base`, or [`FNODE_OFFSET_NULL`] for a pure
///   branch marker (an internal child with no own terminal key; its descendants
///   follow as deeper slots). A terminal+branch internal child emits its own
///   key as a non-NULL offset slot, then its descendants.
///
/// Returns `None` if the subtree can't be conservatively flattened: it's the
/// root (`phys == 0`), contains an `Fnode` child (merging Fnodes is flatten()'s
/// job), exceeds [`FNODE_SLOTS`] array slots, or an offset would collide with
/// the `0xFF` sentinel (subtree spans ≥ 255 `index` positions — can't happen at
/// insert-only density, but guarded defensively). `phys` must be an `Inode`.
fn flatten_subtree_to_fnode<PTR: TrieIndex, LEN: TrieIndex>(
    trie: &NibbleTrie<Vec<u8>, i32, PTR, LEN>,
    phys: usize,
) -> Option<FlatNode<PTR, LEN>> {
    // The root must stay an Inode; only flatten non-root subtrees. The actual
    // build is shared with `NibbleTrie::flatten` via `build_fnode_subtree`.
    if phys == 0 {
        return None;
    }
    trie.build_fnode_subtree(phys)
}

/// Collapse the `Inode` at `phys` into a `FlatNode` (in place — the parent's
/// child slot already points at `phys`, now an `Fnode`). Returns `false` if
/// `phys` is the root (root must stay an `Inode`) or the subtree isn't a
/// conservative Fnode candidate.
fn collapse_to_fnode<PTR: TrieIndex, LEN: TrieIndex>(
    trie: &mut NibbleTrie<Vec<u8>, i32, PTR, LEN>,
    phys: usize,
) -> bool {
    if phys == 0 {
        return false; // root must stay an Inode
    }
    let Some(fnode) = flatten_subtree_to_fnode(&*trie, phys) else {
        return false;
    };
    trie.arena[phys] = ArenaNode::Fnode(fnode);
    true
}

/// First non-root `Inode` in the arena that is a conservative Fnode candidate.
fn first_fnode_candidate<PTR: TrieIndex, LEN: TrieIndex>(
    trie: &NibbleTrie<Vec<u8>, i32, PTR, LEN>,
) -> Option<usize> {
    for phys in 1..trie.arena.len() {
        if !matches!(trie.arena[phys], ArenaNode::Inode(_)) {
            continue;
        }
        if flatten_subtree_to_fnode(trie, phys).is_some() {
            return Some(phys);
        }
    }
    None
}

/// The non-root `Inode` candidate whose flattened Fnode has the *most* slots
/// (ties broken by lowest arena index). Used to pick a deep/chain subtree that
/// actually exercises `flat_get`'s descent path.
fn best_fnode_candidate<PTR: TrieIndex, LEN: TrieIndex>(
    trie: &NibbleTrie<Vec<u8>, i32, PTR, LEN>,
) -> Option<usize> {
    let mut best: Option<(usize, usize)> = None; // (phys, slot_count)
    for phys in 1..trie.arena.len() {
        if !matches!(trie.arena[phys], ArenaNode::Inode(_)) {
            continue;
        }
        if let Some(f) = flatten_subtree_to_fnode(trie, phys) {
            let n = f.slots.len();
            if best.map_or(true, |(_, bn)| n > bn) {
                best = Some((phys, n));
            }
        }
    }
    best.map(|(p, _)| p)
}

/// Cross-check a (possibly Fnode-containing) trie against a `BTreeMap` oracle:
/// `get`, `get_index_unchecked`, forward iteration order, and `seek`
/// lower-bound semantics for every oracle key plus a spread of non-keys.
fn cross_check_fnode<PTR: TrieIndex, LEN: TrieIndex>(
    trie: &NibbleTrie<Vec<u8>, i32, PTR, LEN>,
    oracle: &BTreeMap<Vec<u8>, i32>,
) {
    assert_eq!(trie.len(), oracle.len());
    for (k, v) in oracle {
        assert_eq!(trie.get(k), Some(v), "get mismatch for key {:?}", k);
        #[cfg(feature = "unchecked")]
        {
            // SAFETY: `k` is in the trie (just confirmed above).
            let idx = unsafe { trie.get_index_unchecked(k) }.expect("get_index_unchecked missed a present key");
            assert_eq!(&trie.index[idx].as_ref().unwrap().2, v, "get_index_unchecked value mismatch for {:?}", k);
        }
    }
    // No spurious keys.
    let mut probe = Vec::new();
    for (k, _) in oracle {
        // a few near-miss probes
        probe.push(k.clone());
        let mut shorter = k.clone();
        if shorter.len() > 1 { shorter.truncate(shorter.len() - 1); probe.push(shorter); }
        let mut longer = k.clone(); longer.push(k[0]); probe.push(longer);
    }
    for p in &probe {
        let got = trie.get(p);
        assert_eq!(got.is_some(), oracle.contains_key(p), "get spurious for {:?}", p);
    }

    // Forward iteration == oracle order.
    let mut it = trie.iter();
    let mut ordered = Vec::new();
    if let Some((k, v)) = it.current() { ordered.push((k.to_vec(), *v)); }
    while let Some((k, v)) = it.next() { ordered.push((k.to_vec(), *v)); }
    let expected: Vec<(Vec<u8>, i32)> = oracle.iter().map(|(k, v)| (k.clone(), *v)).collect();
    assert_eq!(ordered, expected, "forward iteration order/value mismatch");

    // Seek lower-bound: for every key plus non-keys, seek(k) == first oracle key >= k.
    let mut seek_probes: Vec<Vec<u8>> = oracle.keys().cloned().collect();
    // Insert inter-key gaps and beyond-ends.
    if oracle.len() >= 2 {
        let mid = oracle.keys().nth(oracle.len() / 2).unwrap();
        let mut gap = mid.clone(); gap[0] = gap[0].wrapping_add(1); seek_probes.push(gap);
    }
    if let Some(last) = oracle.keys().next_back() {
        let mut past = last.clone(); past[0] = past[0].wrapping_add(1); seek_probes.push(past);
        let _ = last;
    }
    seek_probes.push(Vec::new());
    for p in &seek_probes {
        let cursor = trie.iter();
        let mut c = cursor;
        let landed = c.seek(p).map(|(k, _)| k.to_vec());
        let want: Option<Vec<u8>> = oracle.range(p.clone()..).next().map(|(k, _)| k.clone());
        assert_eq!(landed, want, "seek lower-bound mismatch for {:?}", p);
    }
}

#[test]
fn fnode_read_single_level() {
    // "a","b","c","d" all share nib0 (0x6_), diverging at nib1 → root child[6]
    // → one Inode@depth1 with 4 leaf children. Collapsing it yields a 4-entry
    // Fnode (all leaves, no branches): the flat-scan single-level case.
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    let mut oracle = BTreeMap::new();
    for (i, k) in [b"a".to_vec(), b"b".to_vec(), b"c".to_vec(), b"d".to_vec()].iter().enumerate() {
        trie.insert(k.clone(), i as i32).unwrap();
        oracle.insert(k.clone(), i as i32);
    }
    let phys = first_fnode_candidate(&trie).expect("a flat candidate must exist");
    assert!(collapse_to_fnode(&mut trie, phys), "collapse failed");
    assert!(matches!(trie.arena[phys], ArenaNode::Fnode(_)), "arena[phys] is now an Fnode");
    cross_check_fnode(&trie, &oracle);
}

#[test]
fn fnode_read_multi_level_descent() {
    // The trie path-compresses (each Inode discriminates only at the divergence
    // nibble), so a real multi-level Fnode needs keys branching at *different*
    // depths. {"aaaa","aaab","baaa","baab"}: all share nib0 (0x6_); nib1 splits
    // the a-group (nib1=1) from the b-group (nib1=2); each pair then diverges at
    // nib7. The subtree under the nib1 Inode flattens to a 2-branch + 4-leaf
    // (6-slot) Fnode — branch markers followed by deeper leaf entries, which
    // is the layout that exercises flat_get's `can_descend` descent path.
    let keys: [Vec<u8>; 4] = [b"aaaa".to_vec(), b"aaab".to_vec(), b"baaa".to_vec(), b"baab".to_vec()];
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    let mut oracle = BTreeMap::new();
    for (i, k) in keys.iter().enumerate() {
        trie.insert(k.clone(), i as i32).unwrap();
        oracle.insert(k.clone(), i as i32);
    }
    let phys = best_fnode_candidate(&trie).expect("a flat candidate must exist");
    assert!(collapse_to_fnode(&mut trie, phys), "collapse failed");
    if let ArenaNode::Fnode(f) = &trie.arena[phys] {
        let has_branch = f.slots.as_slice().iter().any(|(_, off)| *off == FNODE_OFFSET_NULL);
        assert!(has_branch, "multi-level Fnode must contain a branch marker (descent path)");
        assert_eq!(f.slots.len(), 6, "multi-level Fnode must be 6 slots (2 branches + 4 leaves)");
    } else { panic!("not an Fnode"); }
    cross_check_fnode(&trie, &oracle);
}

#[test]
fn fnode_read_prefix_key() {
    // "aa" is a prefix of "aaaa"/"aaab" — the node where "aa" ends is terminal
    // AND has a child (terminal+branch). The flat scan must still find "aa":
    // it is encoded as a `Some`-ptr edge slot with the deeper leaves following,
    // and flat_get lands on it when the query is exhausted (L==4) and descends
    // past it when the query continues (L==8).
    //
    // The trie path-compresses, so a prefix key lands at the *root* of its local
    // subtree (the divergence node) — and a terminal root can't be collapsed. To
    // get a *non-root* terminal+branch, "b" forces a nib1 split: the nib1 node
    // (non-terminal) has child[1] -> the "aa" subtree (terminal+branch a-node)
    // and child[2] -> leaf "b". Collapsing the nib1 subtree captures the "aa"
    // terminal+branch node as an internal (Some-ptr edge) slot.
    let keys: [Vec<u8>; 4] = [b"aa".to_vec(), b"aaaa".to_vec(), b"aaab".to_vec(), b"b".to_vec()];
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    let mut oracle = BTreeMap::new();
    for (i, k) in keys.iter().enumerate() {
        trie.insert(k.clone(), i as i32).unwrap();
        oracle.insert(k.clone(), i as i32);
    }
    let phys = best_fnode_candidate(&trie).expect("a flat candidate must exist");
    assert!(collapse_to_fnode(&mut trie, phys), "collapse failed");
    // Sanity: the collapsed Fnode contains the prefix key as a terminal+branch
    // slot — a non-NULL offset (terminal) immediately followed by a deeper slot.
    if let ArenaNode::Fnode(f) = &trie.arena[phys] {
        let s = f.slots.as_slice();
        let found_prefix = (0..s.len()).any(|i| {
            s[i].1 != FNODE_OFFSET_NULL && i + 1 < s.len() && s[i + 1].0.as_usize() > s[i].0.as_usize()
        });
        assert!(found_prefix, "expected a terminal+branch (non-NULL offset followed by a deeper) slot");
    } else { panic!("not an Fnode"); }
    cross_check_fnode(&trie, &oracle);
}

#[test]
fn fnode_read_branch_then_leaves() {
    // Keys forming a 2-branch subtree: at some depth two internal children each
    // lead to a small leaf group, so the flattened Fnode has branch markers
    // followed by deeper leaf entries (the motivating 2-level layout).
    // "ax","ay","bx","by": nib0 = 6 for all ('a'/'b' are 0x6_, 'x'/'y' share too);
    // root child[6] → Inode@depth1. nib1: a→1, b→2 → two internal children.
    // Each group {"ax","ay"} / {"bx","by"} diverges at nib2 (x→? y→?):
    //   'x'=0x78 nib0... wait these are byte-indexed: "ax"=[0x61,0x78],
    //   nib0=6,nib1=1; "ay"=[0x61,0x79] nib0=6,nib1=9 → diverge at nib1? No:
    //   "ax" nib1 = 0x78&0xF = 8; "ay" nib1 = 0x79&0xF = 9. So {"ax","ay"} share
    //   nib0=6 only, diverge at nib1 — they're not under a common depth-1 node.
    // Use 3-byte keys instead so each pair shares 2 nibbles then diverges:
    //   "aax"/"aay" share nib0..nib3 (a,a), diverge at nib4 (x/y low nibble).
    let pairs: &[(Vec<u8>, i32)] = &[
        (b"aax".to_vec(), 0), (b"aay".to_vec(), 1),
        (b"bbx".to_vec(), 2), (b"bby".to_vec(), 3),
    ];
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    let mut oracle = BTreeMap::new();
    for (k, v) in pairs {
        trie.insert(k.clone(), *v).unwrap();
        oracle.insert(k.clone(), *v);
    }
    // Find a candidate that actually contains a branch marker (multi-level).
    let phys = first_fnode_candidate(&trie).expect("a flat candidate must exist");
    assert!(collapse_to_fnode(&mut trie, phys), "collapse failed");
    let has_branch = matches!(&trie.arena[phys], ArenaNode::Fnode(f) if f.slots.as_slice().iter().any(|(_, off)| *off == FNODE_OFFSET_NULL));
    assert!(has_branch, "expected the collapsed Fnode to contain a branch marker");
    cross_check_fnode(&trie, &oracle);
}

#[test]
fn fnode_read_stress() {
    // Random keys, collapse every available candidate one at a time, and
    // cross-check after each collapse. A candidate may disappear or appear as
    // the arena changes; we just keep collapsing until none remain.
    let mut state: u64 = 0x9e3779b97f4a7c15;
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    let mut oracle: BTreeMap<Vec<u8>, i32> = BTreeMap::new();
    for _ in 0..200 {
        let k = rand_key(&mut state, 6);
        if oracle.contains_key(&k) {
            continue;
        }
        trie.insert(k.clone(), oracle.len() as i32).unwrap();
        oracle.insert(k, oracle.len() as i32);
    }
    cross_check_fnode(&trie, &oracle); // baseline (all-Inode)
    let mut collapses = 0;
    while let Some(phys) = first_fnode_candidate(&trie) {
        assert!(collapse_to_fnode(&mut trie, phys), "collapse failed at phys {phys}");
        collapses += 1;
        cross_check_fnode(&trie, &oracle);
    }
    assert!(collapses > 0, "stress never collapsed any subtree");
}

#[test]
fn fnode_read_terminal_root() {
    // A subtree whose ROOT is itself terminal (a prefix key with children below)
    // is now flattenable: the root's own key is pulled out into `base`+`terminal:
    // true` and returned by `flat_get`'s FALLBACK (no array slot). "ba" is a
    // prefix of "baaa"/"baab"; "c" forces a nib1 split so the "ba"-rooted subtree
    // is non-root (collapsable). Collapsing it yields a `terminal=true` Fnode
    // whose array holds only the two longer keys (offsets >= 1) — exercising the
    // fallback path that the step-3 root-not-terminal restriction forbade.
    let keys: [Vec<u8>; 4] = [b"baaa".to_vec(), b"baab".to_vec(), b"ba".to_vec(), b"c".to_vec()];
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    let mut oracle = BTreeMap::new();
    for (i, k) in keys.iter().enumerate() {
        trie.insert(k.clone(), i as i32).unwrap();
        oracle.insert(k.clone(), i as i32);
    }
    // Find the terminal-rooted candidate (its flattened Fnode has terminal==true).
    let phys = (1..trie.arena.len())
        .find(|&p| flatten_subtree_to_fnode(&trie, p).map_or(false, |f| f.terminal))
        .expect("a terminal-rooted candidate must exist");
    assert!(collapse_to_fnode(&mut trie, phys), "collapse failed");
    match &trie.arena[phys] {
        ArenaNode::Fnode(f) => {
            assert!(f.terminal, "expected the root's own key pulled out as terminal base");
            assert!(!f.slots.as_slice().is_empty(), "expected descendant array slots");
            // `base` is pulled out of the array (returned by the fallback), so no
            // array slot may carry offset 0 — every slot is either a branch marker
            // (0xFF) or a descendant with offset 1..=254.
            assert!(f.slots.as_slice().iter().all(|(_, off)| *off != 0),
                "terminal-rooted Fnode must not duplicate `base` as an offset-0 slot");
            assert!(f.slots.as_slice().iter().any(|(_, off)| *off != FNODE_OFFSET_NULL),
                "terminal-rooted Fnode must have at least one descendant terminal");
        }
        _ => panic!("not an Fnode"),
    }
    cross_check_fnode(&trie, &oracle);
}

// ── flatten() — the trie produces Fnodes itself ──────────────────────
//
// `flatten()` rebuilds the arena, collapsing qualifying non-root subtrees
// (≤ FNODE_CAP keys, ≥ 2 Inodes) into Fnodes in place of their root. The root
// stays an Inode; key indices are unchanged (arena-only rebuild). These tests
// exercise the real production path (vs the hand-built `collapse_to_fnode`
// read-path tests above) and check idempotence + structural invariants.

#[test]
fn flatten_basic() {
    // {aaaa,aaab,baaa,baab}: the trie path-compresses into a ≥3-Inode subtree
    // (the layout `fnode_read_multi_level_descent` flattens to 6 slots), so
    // `flatten` must emit at least one Fnode, keep the root an Inode, shrink the
    // arena (the subtree's child Inodes are consumed), and stay correct.
    let keys: [Vec<u8>; 4] = [b"aaaa".to_vec(), b"aaab".to_vec(), b"baaa".to_vec(), b"baab".to_vec()];
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    let mut oracle = BTreeMap::new();
    for (i, k) in keys.iter().enumerate() {
        trie.insert(k.clone(), i as i32).unwrap();
        oracle.insert(k.clone(), i as i32);
    }
    trie.optimize();
    let arena_after_opt = trie.arena.len();
    trie.flatten();
    assert!(trie.arena.iter().any(|n| matches!(n, ArenaNode::Fnode(_))),
        "flatten produced no Fnodes");
    assert!(matches!(trie.arena[0], ArenaNode::Inode(_)), "root must stay an Inode");
    assert!(trie.arena.len() < arena_after_opt,
        "arena did not shrink after flatten: {} vs {}", trie.arena.len(), arena_after_opt);
    verify_invariants(&trie).expect("invariants hold after flatten");
    cross_check_fnode(&trie, &oracle);
}

#[test]
fn flatten_idempotent() {
    // Re-flattening an already-flat trie is a no-op topology copy: no new Fnodes
    // appear (existing Fnode children block re-flatten), and correctness holds.
    let keys: [Vec<u8>; 4] = [b"aaaa".to_vec(), b"aaab".to_vec(), b"baaa".to_vec(), b"baab".to_vec()];
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    let mut oracle = BTreeMap::new();
    for (i, k) in keys.iter().enumerate() {
        trie.insert(k.clone(), i as i32).unwrap();
        oracle.insert(k.clone(), i as i32);
    }
    trie.optimize();
    trie.flatten();
    let (fnodes1, len1) = (
        trie.arena.iter().filter(|n| matches!(n, ArenaNode::Fnode(_))).count(),
        trie.arena.len(),
    );
    trie.flatten(); // second pass
    let (fnodes2, len2) = (
        trie.arena.iter().filter(|n| matches!(n, ArenaNode::Fnode(_))).count(),
        trie.arena.len(),
    );
    assert_eq!((fnodes2, len2), (fnodes1, len1), "re-flatten changed the arena");
    verify_invariants(&trie).expect("invariants hold after re-flatten");
    cross_check_fnode(&trie, &oracle);
}

#[test]
fn flatten_stress() {
    // 200 random variable-length keys → optimize → flatten → cross-check. Then
    // re-optimize+flatten (idempotent convergence) and cross-check again.
    let mut state: u64 = 0x123456789abcdef0;
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    let mut oracle: BTreeMap<Vec<u8>, i32> = BTreeMap::new();
    for _ in 0..200 {
        let k = rand_key(&mut state, 6);
        if oracle.contains_key(&k) { continue; }
        trie.insert(k.clone(), oracle.len() as i32).unwrap();
        oracle.insert(k, oracle.len() as i32);
    }
    trie.optimize();
    let arena_after_opt = trie.arena.len();
    trie.flatten();
    assert!(trie.arena.iter().any(|n| matches!(n, ArenaNode::Fnode(_))),
        "stress flatten produced no Fnodes");
    assert!(trie.arena.len() < arena_after_opt, "stress arena did not shrink");
    verify_invariants(&trie).expect("invariants hold after stress flatten");
    cross_check_fnode(&trie, &oracle);

    // Re-optimize over the Fnode-containing arena, then re-flatten. `walk_optimize`
    // remaps Fnode `base`+offsets to fresh `2i+1` slots, so this converges and
    // stays correct. (Wiring `flatten` into `optimize` itself is deferred until
    // step-5 insert handles Fnodes; this exercises the remap path directly.)
    trie.optimize();
    trie.flatten();
    verify_invariants(&trie).expect("invariants hold after re-optimize+flatten");
    cross_check_fnode(&trie, &oracle);
}

#[test]
#[ignore = "memory-footprint sanity; run directly (cargo stdout is filtered)"]
fn flatten_memory_footprint() {
    // Insert 2000 random variable-length keys, optimize, then flatten. Measure
    // the arena's byte footprint (arena.len() * size_of::<ArenaNode>()) before
    // and after flatten to confirm Fnode compaction actually saves memory.
    let mut state: u64 = 0xfeed1234abcd5678;
    let mut trie: NibbleTrie<Vec<u8>, i32> = NibbleTrie::new();
    let mut oracle: BTreeMap<Vec<u8>, i32> = BTreeMap::new();
    for _ in 0..2000 {
        let k = rand_key(&mut state, 6);
        if oracle.contains_key(&k) { continue; }
        trie.insert(k.clone(), oracle.len() as i32).unwrap();
        oracle.insert(k, oracle.len() as i32);
    }
    trie.optimize();
    let before_nodes = trie.arena.len();
    let before_bytes = before_nodes * std::mem::size_of::<ArenaNode<u32, u16>>();
    let fnodes_before = trie.arena.iter().filter(|n| matches!(n, ArenaNode::Fnode(_))).count();
    trie.flatten();
    let after_nodes = trie.arena.len();
    let after_bytes = after_nodes * std::mem::size_of::<ArenaNode<u32, u16>>();
    let fnodes = trie.arena.iter().filter(|n| matches!(n, ArenaNode::Fnode(_))).count();
    let leaves = trie.arena.iter().filter(|n| matches!(n, ArenaNode::Inode(n) if n.children_mask() == 0)).count();
    eprintln!(
        "flatten_memory_footprint: keys={} arena {}->{} nodes ({}B->{}B, {:.1}%), Fnodes {}->{}, leaf-Inodes={}",
        oracle.len(), before_nodes, after_nodes, before_bytes, after_bytes,
        100.0 * after_bytes as f64 / before_bytes as f64, fnodes_before, fnodes, leaves
    );
    assert!(fnodes > 0, "flatten produced no Fnodes");
    assert!(after_bytes < before_bytes, "arena bytes did not drop: {} -> {}", before_bytes, after_bytes);
    verify_invariants(&trie).expect("invariants hold");
    cross_check_fnode(&trie, &oracle);
}

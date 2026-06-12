use super::*;
use std::mem::size_of;

const fn align_up(val: usize, align: usize) -> usize {
    (val + align - 1) & !(align - 1)
}

pub const fn compute_node_size(symbols_len: usize, prefix_len_size: usize, prefix_len_align: usize) -> usize {
    let p_offset = align_up(1, prefix_len_align);
    let symbols_end = p_offset + prefix_len_size + symbols_len;
    let children_offset = align_up(symbols_end, 8);
    children_offset + 8
}

#[test]
fn default_node_is_16_bytes() {
    assert_eq!(size_of::<INode<6, u8>>(), 16);
    assert_eq!(size_of::<PairVec<6, u8>>(), 16);
    assert_eq!(size_of::<Leaf>(), 16);
    assert_eq!(size_of::<Trie<6, u8>>(), 16);
}

#[test]
fn inode_layout_offsets() {
    assert_eq!(std::mem::offset_of!(INode<6, u8>, tag), 0);
    assert_eq!(std::mem::offset_of!(INode<6, u8>, prefix_len), 1);
    assert_eq!(std::mem::offset_of!(INode<6, u8>, symbols), 2);
    assert_eq!(std::mem::offset_of!(INode<6, u8>, children), 8);
}

#[test]
fn pairvec_layout_offsets() {
    assert_eq!(std::mem::offset_of!(PairVec<6, u8>, len), 0);
    assert_eq!(std::mem::offset_of!(PairVec<6, u8>, capacity), 1);
    assert_eq!(std::mem::offset_of!(PairVec<6, u8>, prefix_len), 2);
    assert_eq!(std::mem::offset_of!(PairVec<6, u8>, ptr), 8);
}

#[test]
fn leaf_layout_offsets() {
    assert_eq!(std::mem::offset_of!(Leaf, tag), 0);
    assert_eq!(std::mem::offset_of!(Leaf, payload), 1);
}

#[test]
fn u16_prefix_node_is_24_bytes() {
    assert_eq!(size_of::<INode<6, u16>>(), 24);
    assert_eq!(size_of::<Trie<6, u16>>(), 24);
}

#[test]
fn dense_inline_node_is_24_bytes() {
    assert_eq!(size_of::<INode<14, u8>>(), 24);
    assert_eq!(size_of::<Trie<14, u8>>(), 24);
}

#[test]
fn compute_node_size_matches_inode() {
    assert_eq!(compute_node_size(6, 1, 1), size_of::<INode<6, u8>>());
    assert_eq!(compute_node_size(6, 2, 2), size_of::<INode<6, u16>>());
    assert_eq!(compute_node_size(14, 1, 1), size_of::<INode<14, u8>>());
}

#[test]
fn insert_empty_and_get() {
    let mut trie: TinyTrie<&str, 6, u8> = TinyTrie::new();
    let idx = trie.insert(b"hello".to_vec(), "world").unwrap();
    assert_eq!(idx, 0);
    assert_eq!(trie.get(b"hello\0"), Some(0));
    assert_eq!(trie.get(b"world\0"), None);
}

#[test]
fn insert_duplicate_returns_error() {
    let mut trie: TinyTrie<&str, 6, u8> = TinyTrie::new();
    trie.insert(b"hello".to_vec(), "world").unwrap();
    assert!(trie.insert(b"hello".to_vec(), "other").is_err());
}

#[test]
fn insert_rejects_null_byte() {
    let mut trie: TinyTrie<i32, 6, u8> = TinyTrie::new();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = trie.insert(b"hel\x00lo".to_vec(), 42);
    }));
    assert!(result.is_err());
}

#[test]
fn insert_two_keys_split_leaf() {
    let mut trie: TinyTrie<&str, 6, u8> = TinyTrie::new();
    trie.insert(b"abc".to_vec(), "first").unwrap();
    trie.insert(b"abd".to_vec(), "second").unwrap();
    assert_eq!(trie.get(b"abc\0"), Some(0));
    assert_eq!(trie.get(b"abd\0"), Some(1));
    assert_eq!(trie.get(b"abe\0"), None);
    assert_eq!(trie.get(b"ab\0"), None);
}

#[test]
fn insert_three_keys() {
    let mut trie: TinyTrie<&str, 6, u8> = TinyTrie::new();
    trie.insert(b"abc".to_vec(), "1").unwrap();
    trie.insert(b"abd".to_vec(), "2").unwrap();
    trie.insert(b"abe".to_vec(), "3").unwrap();
    assert_eq!(trie.get(b"abc\0"), Some(0));
    assert_eq!(trie.get(b"abd\0"), Some(1));
    assert_eq!(trie.get(b"abe\0"), Some(2));
    assert_eq!(trie.get(b"abf\0"), None);
}

#[test]
fn insert_prefix_key() {
    let mut trie: TinyTrie<&str, 6, u8> = TinyTrie::new();
    trie.insert(b"abc".to_vec(), "long").unwrap();
    trie.insert(b"ab".to_vec(), "short").unwrap();
    assert_eq!(trie.get(b"abc\0"), Some(0));
    assert_eq!(trie.get(b"ab\0"), Some(1));
    assert_eq!(trie.get(b"abd\0"), None);
}

#[test]
fn insert_reverse_prefix_key() {
    // Insert short key first, then long key.
    let mut trie: TinyTrie<&str, 6, u8> = TinyTrie::new();
    trie.insert(b"ab".to_vec(), "short").unwrap();
    trie.insert(b"abc".to_vec(), "long").unwrap();
    assert_eq!(trie.get(b"ab\0"), Some(0));
    assert_eq!(trie.get(b"abc\0"), Some(1));
}

#[test]
fn insert_no_common_prefix() {
    // Keys with no common prefix.
    let mut trie: TinyTrie<&str, 6, u8> = TinyTrie::new();
    trie.insert(b"abc".to_vec(), "1").unwrap();
    trie.insert(b"xyz".to_vec(), "2").unwrap();
    assert_eq!(trie.get(b"abc\0"), Some(0));
    assert_eq!(trie.get(b"xyz\0"), Some(1));
    assert_eq!(trie.get(b"ab\0"), None);
    assert_eq!(trie.get(b"abcz\0"), None);
}

#[test]
fn insert_single_char_keys() {
    let mut trie: TinyTrie<&str, 6, u8> = TinyTrie::new();
    trie.insert(b"a".to_vec(), "1").unwrap();
    trie.insert(b"b".to_vec(), "2").unwrap();
    trie.insert(b"c".to_vec(), "3").unwrap();
    assert_eq!(trie.get(b"a\0"), Some(0));
    assert_eq!(trie.get(b"b\0"), Some(1));
    assert_eq!(trie.get(b"c\0"), Some(2));
    assert_eq!(trie.get(b"d\0"), None);
}

#[test]
fn insert_many_keys_same_prefix() {
    let mut trie: TinyTrie<usize, 6, u8> = TinyTrie::new();
    for i in 0u8..6 {
        let mut key = b"prefix".to_vec();
        key.push(b'a' + i);
        trie.insert(key, i as usize).unwrap();
    }
    assert_eq!(trie.get(b"prefixa\0"), Some(0));
    assert_eq!(trie.get(b"prefixb\0"), Some(1));
    assert_eq!(trie.get(b"prefixf\0"), Some(5));
    assert_eq!(trie.get(b"prefixg\0"), None);
    assert_eq!(trie.get(b"prefi\0"), None);
}

#[test]
fn insert_deeply_nested() {
    // Insert keys that create a chain of splits.
    let mut trie: TinyTrie<usize, 6, u8> = TinyTrie::new();
    trie.insert(b"a".to_vec(), 0).unwrap();
    trie.insert(b"ab".to_vec(), 1).unwrap();
    trie.insert(b"abc".to_vec(), 2).unwrap();
    trie.insert(b"abcd".to_vec(), 3).unwrap();
    assert_eq!(trie.get(b"a\0"), Some(0));
    assert_eq!(trie.get(b"ab\0"), Some(1));
    assert_eq!(trie.get(b"abc\0"), Some(2));
    assert_eq!(trie.get(b"abcd\0"), Some(3));
    assert_eq!(trie.get(b"abcde\0"), None);
    assert_eq!(trie.get(b"b\0"), None);
}

#[test]
fn insert_branching_at_root() {
    // Keys that diverge at the first byte (no shared prefix).
    let mut trie: TinyTrie<usize, 6, u8> = TinyTrie::new();
    trie.insert(b"aaa".to_vec(), 0).unwrap();
    trie.insert(b"bbb".to_vec(), 1).unwrap();
    trie.insert(b"ccc".to_vec(), 2).unwrap();
    assert_eq!(trie.get(b"aaa\0"), Some(0));
    assert_eq!(trie.get(b"bbb\0"), Some(1));
    assert_eq!(trie.get(b"ccc\0"), Some(2));
    assert_eq!(trie.get(b"ddd\0"), None);
    assert_eq!(trie.get(b"aab\0"), None);
}

#[test]
fn insert_longer_after_shorter() {
    // "ab" then "abcd" — extending beyond an existing prefix.
    let mut trie: TinyTrie<usize, 6, u8> = TinyTrie::new();
    trie.insert(b"ab".to_vec(), 0).unwrap();
    trie.insert(b"abcd".to_vec(), 1).unwrap();
    assert_eq!(trie.get(b"ab\0"), Some(0));
    assert_eq!(trie.get(b"abcd\0"), Some(1));
    assert_eq!(trie.get(b"abc\0"), None);
    assert_eq!(trie.get(b"abcde\0"), None);
}

#[test]
fn insert_promotes_inode_to_pairvec() {
    // With INLINE=4, the 5th child triggers promotion to PairVec.
    let mut trie: TinyTrie<usize, 4, u8> = TinyTrie::new();
    for i in 0u8..7 {
        let mut key = b"prefix".to_vec();
        key.push(b'a' + i);
        trie.insert(key, i as usize).unwrap();
    }
    // All 7 should be findable.
    for i in 0u8..7 {
        let mut key = b"prefix".to_vec();
        key.push(b'a' + i);
        key.push(0); // null-terminate for get()
        assert_eq!(trie.get(&key), Some(i as usize));
    }
    assert_eq!(trie.get(b"prefixh\0"), None);
}

#[test]
fn insert_many_keys_exhausts_inline() {
    // Insert 20 keys with the same prefix (INLINE=6, so PairVec after 6).
    let mut trie: TinyTrie<usize, 6, u8> = TinyTrie::new();
    for i in 0..20 {
        let key = format!("key{:02}", i);
        trie.insert(key.into_bytes(), i).unwrap();
    }
    for i in 0..20 {
        let key = format!("key{:02}", i);
        assert_eq!(trie.get(&null_terminate(key.as_bytes())), Some(i));
    }
    assert_eq!(trie.get(b"key20\0"), None);
    assert_eq!(trie.get(b"key\0"), None);
}

// --- Iterator tests ---

#[test]
fn iter_empty() {
    let trie: TinyTrie<i32, 6, u8> = TinyTrie::new();
    let mut iter = trie.iter();
    assert!(iter.current().is_none());
    assert!(iter.next().is_none());
    assert!(iter.prev().is_none());
}

#[test]
fn iter_single_key() {
    let mut trie: TinyTrie<&str, 6, u8> = TinyTrie::new();
    trie.insert(b"hello".to_vec(), "world").unwrap();
    let mut iter = trie.iter();
    assert_eq!(iter.current(), Some((b"hello".as_slice(), &"world")));
    assert!(iter.next().is_none());
}

#[test]
fn iter_forward() {
    let mut trie: TinyTrie<&str, 6, u8> = TinyTrie::new();
    trie.insert(b"b".to_vec(), "2").unwrap();
    trie.insert(b"d".to_vec(), "4").unwrap();
    trie.insert(b"f".to_vec(), "6").unwrap();
    let mut iter = trie.iter();
    assert_eq!(iter.current(), Some((b"b".as_slice(), &"2")));
    assert_eq!(iter.next(), Some((b"d".as_slice(), &"4")));
    assert_eq!(iter.next(), Some((b"f".as_slice(), &"6")));
    assert!(iter.next().is_none());
}

#[test]
fn iter_backward() {
    let mut trie: TinyTrie<&str, 6, u8> = TinyTrie::new();
    trie.insert(b"b".to_vec(), "2").unwrap();
    trie.insert(b"d".to_vec(), "4").unwrap();
    trie.insert(b"f".to_vec(), "6").unwrap();
    let mut iter = trie.iter();
    // Advance to last key
    iter.next(); // d
    iter.next(); // f
    assert_eq!(iter.current(), Some((b"f".as_slice(), &"6")));
    assert_eq!(iter.prev(), Some((b"d".as_slice(), &"4")));
    assert_eq!(iter.prev(), Some((b"b".as_slice(), &"2")));
    assert!(iter.prev().is_none());
}

#[test]
fn iter_prev_before_first() {
    let mut trie: TinyTrie<&str, 6, u8> = TinyTrie::new();
    trie.insert(b"a".to_vec(), "1").unwrap();
    let mut iter = trie.iter();
    assert_eq!(iter.current(), Some((b"a".as_slice(), &"1")));
    assert!(iter.prev().is_none());
}

#[test]
fn iter_seek_exact() {
    let mut trie: TinyTrie<usize, 6, u8> = TinyTrie::new();
    trie.insert(b"abc".to_vec(), 0).unwrap();
    trie.insert(b"abd".to_vec(), 1).unwrap();
    trie.insert(b"xyz".to_vec(), 2).unwrap();
    let mut iter = trie.iter();
    iter.seek(b"abd\0");
    assert_eq!(iter.current(), Some((b"abd".as_slice(), &1)));
}

#[test]
fn iter_seek_between() {
    let mut trie: TinyTrie<usize, 6, u8> = TinyTrie::new();
    trie.insert(b"abc".to_vec(), 0).unwrap();
    trie.insert(b"xyz".to_vec(), 1).unwrap();
    let mut iter = trie.iter();
    iter.seek(b"mno\0");
    assert_eq!(iter.current(), Some((b"xyz".as_slice(), &1)));
}

#[test]
fn iter_seek_before_all() {
    let mut trie: TinyTrie<usize, 6, u8> = TinyTrie::new();
    trie.insert(b"bbb".to_vec(), 0).unwrap();
    trie.insert(b"ccc".to_vec(), 1).unwrap();
    let mut iter = trie.iter();
    iter.seek(b"aaa\0");
    assert_eq!(iter.current(), Some((b"bbb".as_slice(), &0)));
}

#[test]
fn iter_seek_after_all() {
    let mut trie: TinyTrie<usize, 6, u8> = TinyTrie::new();
    trie.insert(b"bbb".to_vec(), 0).unwrap();
    trie.insert(b"yyy".to_vec(), 1).unwrap();
    let mut iter = trie.iter();
    iter.seek(b"zzz\0");
    assert!(iter.current().is_none());
}

#[test]
fn iter_seek_prefix_key() {
    let mut trie: TinyTrie<usize, 6, u8> = TinyTrie::new();
    trie.insert(b"ab".to_vec(), 0).unwrap();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    let mut iter = trie.iter();
    iter.seek(b"ab\0");
    assert_eq!(iter.current(), Some((b"ab".as_slice(), &0)));
}

#[test]
fn iter_seek_prefix_longer() {
    // Seek "abc" when trie has "ab" and "abcd"
    let mut trie: TinyTrie<usize, 6, u8> = TinyTrie::new();
    trie.insert(b"ab".to_vec(), 0).unwrap();
    trie.insert(b"abcd".to_vec(), 1).unwrap();
    let mut iter = trie.iter();
    iter.seek(b"abc\0");
    assert_eq!(iter.current(), Some((b"abcd".as_slice(), &1)));
}

#[test]
fn iter_seek_then_iterate() {
    let mut trie: TinyTrie<usize, 6, u8> = TinyTrie::new();
    for i in 0..10 {
        let key = format!("key{:02}", i);
        trie.insert(key.into_bytes(), i).unwrap();
    }
    let mut iter = trie.iter();
    iter.seek(b"key05\0");
    assert_eq!(iter.current(), Some((b"key05".as_slice(), &5)));
    // next should go to key06
    assert_eq!(iter.next(), Some((b"key06".as_slice(), &6)));
    // prev should go back to key05
    assert_eq!(iter.prev(), Some((b"key05".as_slice(), &5)));
}

#[test]
fn iter_pairvec() {
    // Force PairVec by inserting >INLINE keys with same prefix (INLINE=6)
    let mut trie: TinyTrie<usize, 6, u8> = TinyTrie::new();
    for i in 0u8..10 {
        let mut key = b"prefix".to_vec();
        key.push(b'a' + i);
        trie.insert(key, i as usize).unwrap();
    }
    // Forward iteration through PairVec
    let mut iter = trie.iter();
    for i in 0u8..10 {
        let mut expected_key = b"prefix".to_vec();
        expected_key.push(b'a' + i);
        assert_eq!(iter.current(), Some((expected_key.as_slice(), &(i as usize))));
        if i < 9 {
            iter.next();
        }
    }
    assert!(iter.next().is_none());

    // Backward iteration through PairVec
    let mut iter2 = trie.iter();
    iter2.seek(b"prefixj\0"); // last key
    for i in (0..10).rev() {
        let mut expected_key = b"prefix".to_vec();
        expected_key.push(b'a' + i as u8);
        assert_eq!(iter2.current(), Some((expected_key.as_slice(), &(i as usize))));
        if i > 0 {
            iter2.prev();
        }
    }
    assert!(iter2.prev().is_none());
}

#[test]
fn iter_deeply_nested() {
    let mut trie: TinyTrie<usize, 6, u8> = TinyTrie::new();
    trie.insert(b"a".to_vec(), 0).unwrap();
    trie.insert(b"ab".to_vec(), 1).unwrap();
    trie.insert(b"abc".to_vec(), 2).unwrap();
    trie.insert(b"abcd".to_vec(), 3).unwrap();
    // Forward
    let mut iter = trie.iter();
    assert_eq!(iter.current(), Some((b"a".as_slice(), &0)));
    assert_eq!(iter.next(), Some((b"ab".as_slice(), &1)));
    assert_eq!(iter.next(), Some((b"abc".as_slice(), &2)));
    assert_eq!(iter.next(), Some((b"abcd".as_slice(), &3)));
    assert!(iter.next().is_none());
    // Backward
    assert_eq!(iter.prev(), Some((b"abc".as_slice(), &2)));
    assert_eq!(iter.prev(), Some((b"ab".as_slice(), &1)));
    assert_eq!(iter.prev(), Some((b"a".as_slice(), &0)));
    assert!(iter.prev().is_none());
}

#[test]
fn iter_full_sort_order() {
    // Insert keys in random order, verify iteration returns them sorted
    let mut trie: TinyTrie<usize, 6, u8> = TinyTrie::new();
    let keys: Vec<Vec<u8>> = vec![
        b"delta".to_vec(), b"alpha".to_vec(), b"charlie".to_vec(),
        b"bravo".to_vec(), b"echo".to_vec(),
    ];
    for (i, k) in keys.iter().enumerate() {
        trie.insert(k.clone(), i).unwrap();
    }
    let mut iter = trie.iter();
    let mut collected: Vec<Vec<u8>> = Vec::new();
    loop {
        if let Some((k, _)) = iter.current() {
            collected.push(k.to_vec());
        }
        if iter.next().is_none() { break; }
    }
    let expected: Vec<&[u8]> = vec![b"alpha", b"bravo", b"charlie", b"delta", b"echo"];
    assert_eq!(collected, expected);
}

#[test]
fn iter_many_keys_forward() {
    // Insert 200 keys and iterate all of them forward to test for hangs
    let mut trie: TinyTrie<usize, 6, u8> = TinyTrie::new();
    let n = 200;
    for i in 0..n {
        let key = format!("key_{:04}", i);
        trie.insert(key.into_bytes(), i).unwrap();
    }

    let mut iter = trie.iter();
    let mut count = 0;
    let mut last_key: Option<Vec<u8>> = None;

    loop {
        if let Some((k, v)) = iter.current() {
            // Verify iteration is in sorted order
            if let Some(ref prev) = last_key {
                assert!(k > prev.as_slice(), "iteration not in sorted order: {:?} <= {:?}", k, prev);
            }
            assert_eq!(*v, count, "value mismatch at key {:?}", k);
            last_key = Some(k.to_vec());
            count += 1;
        } else {
            // current() returned None before next() — only valid if exhausted
        }
        if iter.next().is_none() {
            break;
        }
        // Safety valve: if we iterate more than n times, something is wrong
        assert!(count <= n, "iterated more than {} times, likely infinite loop", n);
    }

    assert_eq!(count, n, "expected {} iterations, got {}", n, count);
}

#[test]
fn iter_many_keys_backward() {
    // Insert 200 keys, seek to end, iterate backward
    let mut trie: TinyTrie<usize, 6, u8> = TinyTrie::new();
    let n = 200;
    for i in 0..n {
        let key = format!("key_{:04}", i);
        trie.insert(key.into_bytes(), i).unwrap();
    }

    // Seek to the last key
    let mut iter = trie.iter();
    iter.seek(&null_terminate(format!("key_{:04}", n - 1).as_bytes()));
    assert_eq!(iter.current().map(|(k, _)| k.to_vec()), Some(format!("key_{:04}", n - 1).into_bytes()));

    // Iterate backward
    let mut count = 1; // already at the last key
    loop {
        if iter.prev().is_none() {
            break;
        }
        count += 1;
        assert!(count <= n, "iterated more than {} times backward, likely infinite loop", n);
    }
    assert_eq!(count, n, "expected {} backward iterations, got {}", n, count);
}

#[test]
fn iter_seek_and_scan_forward() {
    // Insert 100 keys, seek to middle, scan forward to end
    let mut trie: TinyTrie<usize, 6, u8> = TinyTrie::new();
    let n = 100;
    for i in 0..n {
        let key = format!("key_{:04}", i);
        trie.insert(key.into_bytes(), i).unwrap();
    }

    let mut iter = trie.iter();
    let start = 50;
    iter.seek(&null_terminate(format!("key_{:04}", start).as_bytes()));
    assert_eq!(iter.current(), Some((format!("key_{:04}", start).as_bytes(), &start)));

    let mut count = 0;
    let mut expected = start;
    loop {
        if let Some((k, v)) = iter.current() {
            assert_eq!(k, format!("key_{:04}", expected).as_bytes());
            assert_eq!(*v, expected);
            count += 1;
            expected += 1;
        }
        if iter.next().is_none() { break; }
        assert!(count <= n - start, "too many iterations");
    }
    assert_eq!(count, n - start);
}

#[test]
fn iter_last_and_backward() {
    // iter_last() positions at the last key, then iterate backward
    let mut trie: TinyTrie<usize, 6, u8> = TinyTrie::new();
    let n = 50;
    for i in 0..n {
        let key = format!("key_{:04}", i);
        trie.insert(key.into_bytes(), i).unwrap();
    }

    let mut iter = trie.iter_last();
    assert_eq!(iter.current(), Some((format!("key_{:04}", n - 1).as_bytes(), &(n - 1))));

    // Iterate backward
    let mut count = 1; // already at last key
    while iter.prev().is_some() {
        count += 1;
        assert!(count <= n, "too many backward iterations");
    }
    assert_eq!(count, n, "expected {} iterations, got {}", n, count);
}

#[test]
fn len_and_is_empty() {
    let mut trie: TinyTrie<&str, 6, u8> = TinyTrie::new();
    assert!(trie.is_empty());
    assert_eq!(trie.len(), 0);

    trie.insert(b"hello".to_vec(), "world").unwrap();
    assert!(!trie.is_empty());
    assert_eq!(trie.len(), 1);

    trie.insert(b"abc".to_vec(), "def").unwrap();
    assert_eq!(trie.len(), 2);
}

#[test]
fn get_value_found_and_missing() {
    let mut trie: TinyTrie<&str, 6, u8> = TinyTrie::new();
    trie.insert(b"hello".to_vec(), "world").unwrap();
    trie.insert(b"abc".to_vec(), "def").unwrap();

    assert_eq!(trie.get_value(b"hello\0"), Some(&"world"));
    assert_eq!(trie.get_value(b"abc\0"), Some(&"def"));
    assert_eq!(trie.get_value(b"xyz\0"), None);
}

#[test]
fn get_value_empty_trie() {
    let trie: TinyTrie<i32, 6, u8> = TinyTrie::new();
    assert_eq!(trie.get_value(b"anything\0"), None);
}

#[test]
fn into_keys_values_roundtrip() {
    let mut trie: TinyTrie<i32, 6, u8> = TinyTrie::new();
    trie.insert(b"alpha".to_vec(), 1).unwrap();
    trie.insert(b"bravo".to_vec(), 2).unwrap();
    trie.insert(b"charlie".to_vec(), 3).unwrap();

    let (keys, values) = trie.into_keys_values();
    // Keys include the null terminator
    assert_eq!(keys.len(), 3);
    assert_eq!(values.len(), 3);
    // Values correspond to insertion order
    assert_eq!(values[0], 1);
    assert_eq!(values[1], 2);
    assert_eq!(values[2], 3);
}

#[test]
fn into_keys_values_empty() {
    let trie: TinyTrie<i32, 6, u8> = TinyTrie::new();
    let (keys, values) = trie.into_keys_values();
    assert!(keys.is_empty());
    assert!(values.is_empty());
}

#[test]
fn into_keys_values_no_double_free() {
    // This test verifies that into_keys_values does not double-free.
    // If it did, this would trigger Miri errors or ASan reports.
    let mut trie: TinyTrie<String, 6, u8> = TinyTrie::new();
    for i in 0..20 {
        let key = format!("key_{:04}", i);
        trie.insert(key.into_bytes(), format!("val_{}", i)).unwrap();
    }
    let (keys, values) = trie.into_keys_values();
    assert_eq!(keys.len(), 20);
    assert_eq!(values.len(), 20);
}
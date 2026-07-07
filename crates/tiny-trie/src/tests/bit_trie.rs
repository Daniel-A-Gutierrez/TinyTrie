use super::*;

#[test]
fn node_size() {
    assert_eq!(std::mem::size_of::<Node>(), 16);
}

#[test]
fn insert_empty_and_get() {
    let mut trie: BitTrie<Vec<u8>, i32> = BitTrie::new();
    trie.insert(b"hello".to_vec(), 42).unwrap();
    assert_eq!(trie.get(b"hello"), Some(&42));
    assert_eq!(trie.get(b"world"), None);
}

#[test]
fn insert_duplicate_returns_error() {
    let mut trie: BitTrie<Vec<u8>, i32> = BitTrie::new();
    trie.insert(b"hello".to_vec(), 1).unwrap();
    let result = trie.insert(b"hello".to_vec(), 2);
    assert_eq!(result, Err(()));
    assert_eq!(trie.len(), 1);
}

#[test]
fn insert_null_byte_allowed() {
    let mut trie: BitTrie<Vec<u8>, i32> = BitTrie::new();
    trie.insert(b"hel\0lo".to_vec(), 1).unwrap();
    assert_eq!(trie.get(b"hel\0lo"), Some(&1));
}

#[test]
fn insert_two_keys_split_leaf() {
    let mut trie: BitTrie<Vec<u8>, i32> = BitTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"abd".to_vec(), 2).unwrap();
    assert_eq!(trie.get(b"abc"), Some(&1));
    assert_eq!(trie.get(b"abd"), Some(&2));
    assert_eq!(trie.len(), 2);
}

#[test]
fn insert_prefix_key() {
    let mut trie: BitTrie<Vec<u8>, i32> = BitTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"abcd".to_vec(), 2).unwrap();
    assert_eq!(trie.get(b"abc"), Some(&1));
    assert_eq!(trie.get(b"abcd"), Some(&2));
}

#[test]
fn insert_reverse_prefix_key() {
    let mut trie: BitTrie<Vec<u8>, i32> = BitTrie::new();
    trie.insert(b"abcd".to_vec(), 1).unwrap();
    trie.insert(b"abc".to_vec(), 2).unwrap();
    assert_eq!(trie.get(b"abcd"), Some(&1));
    assert_eq!(trie.get(b"abc"), Some(&2));
}

#[test]
fn insert_empty_key() {
    let mut trie: BitTrie<Vec<u8>, i32> = BitTrie::new();
    trie.insert(b"".to_vec(), 1).unwrap();
    trie.insert(b"abc".to_vec(), 2).unwrap();
    assert_eq!(trie.get(b""), Some(&1));
    assert_eq!(trie.get(b"abc"), Some(&2));
}

#[test]
fn insert_empty_key_after() {
    let mut trie: BitTrie<Vec<u8>, i32> = BitTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"".to_vec(), 2).unwrap();
    assert_eq!(trie.get(b"abc"), Some(&1));
    assert_eq!(trie.get(b""), Some(&2));
}

#[test]
fn insert_no_common_prefix() {
    let mut trie: BitTrie<Vec<u8>, i32> = BitTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"xyz".to_vec(), 2).unwrap();
    assert_eq!(trie.get(b"abc"), Some(&1));
    assert_eq!(trie.get(b"xyz"), Some(&2));
}

#[test]
fn insert_three_keys() {
    let mut trie: BitTrie<Vec<u8>, i32> = BitTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"abd".to_vec(), 2).unwrap();
    trie.insert(b"abe".to_vec(), 3).unwrap();
    assert_eq!(trie.get(b"abc"), Some(&1));
    assert_eq!(trie.get(b"abd"), Some(&2));
    assert_eq!(trie.get(b"abe"), Some(&3));
}

#[test]
fn insert_single_char_keys() {
    let mut trie: BitTrie<Vec<u8>, i32> = BitTrie::new();
    for c in b'a'..=b'f' {
        trie.insert(vec![c], c as i32).unwrap();
    }
    for c in b'a'..=b'f' {
        let v = c as i32;
        assert_eq!(trie.get(&[c]), Some(&v));
    }
}

#[test]
fn insert_many_keys_same_prefix() {
    let mut trie: BitTrie<Vec<u8>, i32> = BitTrie::new();
    for i in 0..50 {
        let key = format!("prefix_{:02}", i);
        trie.insert(key.into_bytes(), i).unwrap();
    }
    for i in 0..50 {
        let key = format!("prefix_{:02}", i);
        let result = trie.get(key.as_bytes());
        assert!(result.is_some(), "get({:?}) returned None for i={}", key, i);
    }
}

#[test]
fn insert_deeply_nested() {
    let mut trie: BitTrie<Vec<u8>, i32> = BitTrie::new();
    let mut key = Vec::new();
    for i in 0..100 {
        key.push(b'a');
        trie.insert(key.clone(), i).unwrap();
        assert_eq!(trie.get(&key), Some(&i));
    }
}

#[test]
fn len_and_is_empty() {
    let mut trie: BitTrie<Vec<u8>, i32> = BitTrie::new();
    assert!(trie.is_empty());
    assert_eq!(trie.len(), 0);
    trie.insert(b"hello".to_vec(), 1).unwrap();
    assert!(!trie.is_empty());
    assert_eq!(trie.len(), 1);
}

#[test]
fn into_keys_values_roundtrip() {
    let mut trie: BitTrie<Vec<u8>, i32> = BitTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"def".to_vec(), 2).unwrap();
    let (keys, values) = trie.into_keys_values();
    assert_eq!(keys, vec![b"abc".to_vec(), b"def".to_vec()]);
    assert_eq!(values, vec![1, 2]);
}

#[test]
fn iter_empty() {
    let trie: BitTrie<Vec<u8>, i32> = BitTrie::new();
    let mut iter = trie.iter();
    assert!(iter.next().is_none());
}

#[test]
fn iter_single_key() {
    let mut trie: BitTrie<Vec<u8>, i32> = BitTrie::new();
    trie.insert(b"hello".to_vec(), 42).unwrap();
    let mut iter = trie.iter();
    let (k, v) = iter.next().unwrap();
    assert_eq!(k, b"hello");
    assert_eq!(*v, 42);
    assert!(iter.next().is_none());
}

#[test]
fn iter_forward() {
    let mut trie: BitTrie<Vec<u8>, i32> = BitTrie::new();
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
    let mut trie: BitTrie<Vec<u8>, i32> = BitTrie::new();
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
fn iter_with_prefix_key() {
    let mut trie: BitTrie<Vec<u8>, i32> = BitTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"abcd".to_vec(), 2).unwrap();

    let mut results = Vec::new();
    let mut iter = trie.iter();
    while let Some((k, _)) = iter.next() {
        results.push(k.to_vec());
    }
    assert_eq!(results, vec![b"abc".to_vec(), b"abcd".to_vec()]);
}

#[test]
fn iter_with_empty_key() {
    let mut trie: BitTrie<Vec<u8>, i32> = BitTrie::new();
    trie.insert(b"".to_vec(), 0).unwrap();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"def".to_vec(), 2).unwrap();

    let mut results = Vec::new();
    let mut iter = trie.iter();
    while let Some((k, _)) = iter.next() {
        results.push(k.to_vec());
    }
    // Empty string sorts before everything
    assert_eq!(results, vec![b"".to_vec(), b"abc".to_vec(), b"def".to_vec()]);
}

#[test]
fn iter_seek_exact() {
    let mut trie: BitTrie<Vec<u8>, i32> = BitTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"abd".to_vec(), 2).unwrap();
    trie.insert(b"abe".to_vec(), 3).unwrap();

    let mut iter = trie.iter();
    let (k, _) = iter.seek(b"abd").unwrap();
    assert_eq!(k, b"abd");
}

#[test]
fn iter_seek_between() {
    let mut trie: BitTrie<Vec<u8>, i32> = BitTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"abd".to_vec(), 2).unwrap();
    trie.insert(b"abe".to_vec(), 3).unwrap();

    let mut iter = trie.iter();
    let (k, _) = iter.seek(b"abc\x7f").unwrap();
    assert_eq!(k, b"abd");
}

#[test]
fn iter_seek_prefix_key() {
    let mut trie: BitTrie<Vec<u8>, i32> = BitTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"abcd".to_vec(), 2).unwrap();

    let mut iter = trie.iter();
    let (k, _) = iter.seek(b"abc").unwrap();
    assert_eq!(k, b"abc");
}

#[test]
fn get_found_and_missing() {
    let mut trie: BitTrie<Vec<u8>, String> = BitTrie::new();
    trie.insert(b"hello".to_vec(), "world".to_string()).unwrap();
    assert_eq!(trie.get(b"hello"), Some(&"world".to_string()));
    assert_eq!(trie.get(b"world"), None);
}

#[test]
fn iter_backward_large() {
    let mut trie: BitTrie<Vec<u8>, i32> = BitTrie::new();
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
fn string_key_insert_and_get() {
    let mut trie: BitTrie<String, i32> = BitTrie::new();
    trie.insert("hello".to_string(), 1).unwrap();
    trie.insert("world".to_string(), 2).unwrap();
    assert_eq!(trie.get(b"hello"), Some(&1));
    assert_eq!(trie.get(b"world"), Some(&2));
    assert_eq!(trie.get(b"xyz"), None);
}

#[test]
fn string_key_prefix() {
    let mut trie: BitTrie<String, i32> = BitTrie::new();
    trie.insert("abc".to_string(), 1).unwrap();
    trie.insert("abcd".to_string(), 2).unwrap();
    assert_eq!(trie.get(b"abc"), Some(&1));
    assert_eq!(trie.get(b"abcd"), Some(&2));
}

#[test]
fn string_key_into_keys_values() {
    let mut trie: BitTrie<String, i32> = BitTrie::new();
    trie.insert("abc".to_string(), 1).unwrap();
    trie.insert("def".to_string(), 2).unwrap();
    let (keys, values) = trie.into_keys_values();
    assert_eq!(keys, vec!["abc".to_string(), "def".to_string()]);
    assert_eq!(values, vec![1, 2]);
}

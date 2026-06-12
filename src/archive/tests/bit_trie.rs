use super::*;

#[test]
fn node_size() {
    assert_eq!(std::mem::size_of::<Node>(), 12);
}

#[test]
fn insert_empty_and_get() {
    let mut trie: BitTrie<i32> = BitTrie::new();
    let idx = trie.insert(b"hello".to_vec(), 42).unwrap();
    assert_eq!(trie.get(b"hello\0"), Some(idx));
    assert_eq!(trie.get_value(b"hello\0"), Some(&42));
    assert_eq!(trie.get(b"world\0"), None);
}

#[test]
fn insert_duplicate_returns_error() {
    let mut trie: BitTrie<i32> = BitTrie::new();
    trie.insert(b"hello".to_vec(), 1).unwrap();
    let result = trie.insert(b"hello".to_vec(), 2);
    assert_eq!(result, Err(()));
    assert_eq!(trie.len(), 1);
}

#[test]
fn insert_rejects_null_byte() {
    let mut trie: BitTrie<i32> = BitTrie::new();
    let result = trie.insert(b"hel\0lo".to_vec(), 1);
    assert_eq!(result, Err(()));
}

#[test]
fn insert_two_keys_split_leaf() {
    let mut trie: BitTrie<i32> = BitTrie::new();
    let i1 = trie.insert(b"abc".to_vec(), 1).unwrap();
    let i2 = trie.insert(b"abd".to_vec(), 2).unwrap();
    assert_eq!(trie.get(b"abc\0"), Some(i1));
    assert_eq!(trie.get(b"abd\0"), Some(i2));
    assert_eq!(trie.len(), 2);
}

#[test]
fn insert_prefix_key() {
    let mut trie: BitTrie<i32> = BitTrie::new();
    let i1 = trie.insert(b"abc".to_vec(), 1).unwrap();
    let i2 = trie.insert(b"abcd".to_vec(), 2).unwrap();
    assert_eq!(trie.get(b"abc\0"), Some(i1));
    assert_eq!(trie.get(b"abcd\0"), Some(i2));
}

#[test]
fn insert_reverse_prefix_key() {
    let mut trie: BitTrie<i32> = BitTrie::new();
    let i1 = trie.insert(b"abcd".to_vec(), 1).unwrap();
    let i2 = trie.insert(b"abc".to_vec(), 2).unwrap();
    assert_eq!(trie.get(b"abcd\0"), Some(i1));
    assert_eq!(trie.get(b"abc\0"), Some(i2));
}

#[test]
fn insert_no_common_prefix() {
    let mut trie: BitTrie<i32> = BitTrie::new();
    let i1 = trie.insert(b"abc".to_vec(), 1).unwrap();
    let i2 = trie.insert(b"xyz".to_vec(), 2).unwrap();
    assert_eq!(trie.get(b"abc\0"), Some(i1));
    assert_eq!(trie.get(b"xyz\0"), Some(i2));
}

#[test]
fn insert_three_keys() {
    let mut trie: BitTrie<i32> = BitTrie::new();
    let i1 = trie.insert(b"abc".to_vec(), 1).unwrap();
    let i2 = trie.insert(b"abd".to_vec(), 2).unwrap();
    let i3 = trie.insert(b"abe".to_vec(), 3).unwrap();
    assert_eq!(trie.get(b"abc\0"), Some(i1));
    assert_eq!(trie.get(b"abd\0"), Some(i2));
    assert_eq!(trie.get(b"abe\0"), Some(i3));
}

#[test]
fn insert_single_char_keys() {
    let mut trie: BitTrie<i32> = BitTrie::new();
    let mut indices = Vec::new();
    for c in b'a'..=b'f' {
        let idx = trie.insert(vec![c], c as i32).unwrap();
        indices.push(idx);
    }
    for (i, c) in (b'a'..=b'f').enumerate() {
        let key = vec![c, 0];
        assert_eq!(trie.get(&key), Some(indices[i]));
    }
}

#[test]
fn insert_many_keys_same_prefix() {
    let mut trie: BitTrie<i32> = BitTrie::new();
    for i in 0..50 {
        let key = format!("prefix_{:02}", i);
        trie.insert(key.into_bytes(), i).unwrap();
    }
    for i in 0..50 {
        let key = format!("prefix_{:02}\0", i);
        let result = trie.get(key.as_bytes());
        assert!(result.is_some(), "get({:?}) returned None for i={}", key, i);
    }
}

#[test]
fn insert_deeply_nested() {
    let mut trie: BitTrie<i32> = BitTrie::new();
    let mut key = Vec::new();
    for i in 0..100 {
        key.push(b'a');
        let idx = trie.insert(key.clone(), i).unwrap();
        let mut nt_key = key.clone();
        nt_key.push(0);
        assert_eq!(trie.get(&nt_key), Some(idx));
    }
}

#[test]
fn len_and_is_empty() {
    let mut trie: BitTrie<i32> = BitTrie::new();
    assert!(trie.is_empty());
    assert_eq!(trie.len(), 0);
    trie.insert(b"hello".to_vec(), 1).unwrap();
    assert!(!trie.is_empty());
    assert_eq!(trie.len(), 1);
}

#[test]
fn into_keys_values_roundtrip() {
    let mut trie: BitTrie<i32> = BitTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"def".to_vec(), 2).unwrap();
    let (keys, values) = trie.into_keys_values();
    assert_eq!(keys, vec![b"abc".to_vec(), b"def".to_vec()]);
    assert_eq!(values, vec![1, 2]);
}

#[test]
fn iter_empty() {
    let trie: BitTrie<i32> = BitTrie::new();
    let mut iter = trie.iter();
    assert!(iter.next().is_none());
}

#[test]
fn iter_single_key() {
    let mut trie: BitTrie<i32> = BitTrie::new();
    trie.insert(b"hello".to_vec(), 42).unwrap();
    let mut iter = trie.iter();
    let (k, v) = iter.next().unwrap();
    assert_eq!(k, b"hello");
    assert_eq!(*v, 42);
    assert!(iter.next().is_none());
}

#[test]
fn iter_forward() {
    let mut trie: BitTrie<i32> = BitTrie::new();
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
    let mut trie: BitTrie<i32> = BitTrie::new();
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
    let mut trie: BitTrie<i32> = BitTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"abd".to_vec(), 2).unwrap();
    trie.insert(b"abe".to_vec(), 3).unwrap();

    let mut iter = trie.iter();
    let (k, _) = iter.seek(b"abd\0").unwrap();
    assert_eq!(k, b"abd");
}

#[test]
fn iter_seek_between() {
    let mut trie: BitTrie<i32> = BitTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"abd".to_vec(), 2).unwrap();
    trie.insert(b"abe".to_vec(), 3).unwrap();

    let mut iter = trie.iter();
    let (k, _) = iter.seek(b"abc\x7f\0").unwrap();
    assert_eq!(k, b"abd");
}

#[test]
fn iter_seek_prefix_key() {
    let mut trie: BitTrie<i32> = BitTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"abcd".to_vec(), 2).unwrap();

    let mut iter = trie.iter();
    let (k, _) = iter.seek(b"abc\0").unwrap();
    assert_eq!(k, b"abc");
}

#[test]
fn get_value_found_and_missing() {
    let mut trie: BitTrie<String> = BitTrie::new();
    trie.insert(b"hello".to_vec(), "world".to_string()).unwrap();
    assert_eq!(trie.get_value(b"hello\0"), Some(&"world".to_string()));
    assert_eq!(trie.get_value(b"world\0"), None);
}

#[test]
fn iter_backward_large() {
    let mut trie: BitTrie<i32> = BitTrie::new();
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
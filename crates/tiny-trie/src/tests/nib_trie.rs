use super::*;

#[test]
fn node_size_default() {
    // Default PTR=u32, LEN=u16: 4*4 + 4 + 2 + 3*1 = 25 → padded to 28
    assert_eq!(std::mem::size_of::<NibNode<u32, u16>>(), 28);
}

#[test]
fn node_size_compact() {
    // Compact PTR=u16, LEN=u16: 4*2 + 2 + 2 + 3*1 = 13 → padded to 16
    assert_eq!(std::mem::size_of::<NibNode<u16, u16>>(), 16);
}

#[test]
fn insert_empty_and_get() {
    let mut trie: NibTrie<i32> = NibTrie::new();
    trie.insert(b"hello".to_vec(), 42).unwrap();
    assert_eq!(trie.get(b"hello"), Some(&42));
    assert_eq!(trie.get(b"world"), None);
}

#[test]
fn insert_duplicate_returns_error() {
    let mut trie: NibTrie<i32> = NibTrie::new();
    trie.insert(b"hello".to_vec(), 1).unwrap();
    let result = trie.insert(b"hello".to_vec(), 2);
    assert_eq!(result, Err(()));
    assert_eq!(trie.len(), 1);
}

#[test]
fn insert_null_byte_allowed() {
    let mut trie: NibTrie<i32> = NibTrie::new();
    trie.insert(b"hel\0lo".to_vec(), 1).unwrap();
    assert_eq!(trie.get(b"hel\0lo"), Some(&1));
}

#[test]
fn insert_two_keys_split_leaf() {
    let mut trie: NibTrie<i32> = NibTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"abd".to_vec(), 2).unwrap();
    assert_eq!(trie.get(b"abc"), Some(&1));
    assert_eq!(trie.get(b"abd"), Some(&2));
    assert_eq!(trie.len(), 2);
}

#[test]
fn insert_prefix_key() {
    let mut trie: NibTrie<i32> = NibTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"abcd".to_vec(), 2).unwrap();
    assert_eq!(trie.get(b"abc"), Some(&1));
    assert_eq!(trie.get(b"abcd"), Some(&2));
}

#[test]
fn insert_reverse_prefix_key() {
    let mut trie: NibTrie<i32> = NibTrie::new();
    trie.insert(b"abcd".to_vec(), 1).unwrap();
    trie.insert(b"abc".to_vec(), 2).unwrap();
    assert_eq!(trie.get(b"abcd"), Some(&1));
    assert_eq!(trie.get(b"abc"), Some(&2));
}

#[test]
fn insert_no_common_prefix() {
    let mut trie: NibTrie<i32> = NibTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"xyz".to_vec(), 2).unwrap();
    assert_eq!(trie.get(b"abc"), Some(&1));
    assert_eq!(trie.get(b"xyz"), Some(&2));
}

#[test]
fn insert_three_keys() {
    let mut trie: NibTrie<i32> = NibTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"abd".to_vec(), 2).unwrap();
    trie.insert(b"abx".to_vec(), 3).unwrap();
    assert_eq!(trie.get(b"abc"), Some(&1));
    assert_eq!(trie.get(b"abd"), Some(&2));
    assert_eq!(trie.get(b"abx"), Some(&3));
    assert_eq!(trie.len(), 3);
}

#[test]
fn insert_empty_key() {
    let mut trie: NibTrie<i32> = NibTrie::new();
    trie.insert(b"".to_vec(), 42).unwrap();
    assert_eq!(trie.get(b""), Some(&42));
    assert_eq!(trie.get(b"a"), None);
}

#[test]
fn insert_single_byte_keys() {
    let mut trie: NibTrie<i32> = NibTrie::new();
    trie.insert(b"\x00".to_vec(), 0).unwrap();
    trie.insert(b"\x01".to_vec(), 1).unwrap();
    trie.insert(b"\x02".to_vec(), 2).unwrap();
    trie.insert(b"\x03".to_vec(), 3).unwrap();
    trie.insert(b"\x04".to_vec(), 4).unwrap();
    assert_eq!(trie.get(b"\x00"), Some(&0));
    assert_eq!(trie.get(b"\x01"), Some(&1));
    assert_eq!(trie.get(b"\x02"), Some(&2));
    assert_eq!(trie.get(b"\x03"), Some(&3));
    assert_eq!(trie.get(b"\x04"), Some(&4));
    assert_eq!(trie.len(), 5);
}

#[test]
fn insert_many_sequential() {
    let mut trie: NibTrie<i32> = NibTrie::new();
    let n = 1000;
    for i in 0..n {
        let key = format!("key_{:04}", i).into_bytes();
        trie.insert(key, i).unwrap();
    }
    assert_eq!(trie.len(), n as usize);
    for i in 0..n {
        let key = format!("key_{:04}", i).into_bytes();
        assert_eq!(trie.get(&key), Some(&i));
    }
    assert_eq!(trie.get(b"key_9999"), None);
}

#[test]
fn iteration_forward() {
    let mut trie: NibTrie<i32> = NibTrie::new();
    let keys = vec![b"abc".to_vec(), b"abd".to_vec(), b"abe".to_vec(), b"xyz".to_vec()];
    for (i, k) in keys.iter().enumerate() {
        trie.insert(k.clone(), i as i32).unwrap();
    }
    let mut it = trie.iter();
    let mut result = Vec::new();
    if let Some((k, v)) = it.current() {
        result.push((k.to_vec(), *v));
    }
    while let Some((k, v)) = it.next() {
        result.push((k.to_vec(), *v));
    }
    assert_eq!(result.len(), 4);
    // Should be in sorted order
    for i in 1..result.len() {
        assert!(result[i - 1].0 < result[i].0);
    }
}

#[test]
fn iteration_backward() {
    let mut trie: NibTrie<i32> = NibTrie::new();
    let keys = vec![b"abc".to_vec(), b"abd".to_vec(), b"abe".to_vec(), b"xyz".to_vec()];
    for (i, k) in keys.iter().enumerate() {
        trie.insert(k.clone(), i as i32).unwrap();
    }
    let mut it = trie.iter_last();
    let mut result = Vec::new();
    if let Some((k, v)) = it.current() {
        result.push((k.to_vec(), *v));
    }
    while let Some((k, v)) = it.prev() {
        result.push((k.to_vec(), *v));
    }
    assert_eq!(result.len(), 4);
    // Should be in reverse sorted order
    for i in 1..result.len() {
        assert!(result[i - 1].0 > result[i].0);
    }
}

#[test]
fn optimize_preserves_keys() {
    let mut trie: NibTrie<i32> = NibTrie::new();
    let n = 100;
    for i in 0..n {
        let key = format!("key_{:04}", i).into_bytes();
        trie.insert(key, i).unwrap();
    }
    trie.optimize();
    assert_eq!(trie.len(), n as usize);
    for i in 0..n {
        let key = format!("key_{:04}", i).into_bytes();
        assert_eq!(trie.get(&key), Some(&i));
    }
}

#[test]
fn seek_basic() {
    let mut trie: NibTrie<i32> = NibTrie::new();
    for i in 0..10 {
        let key = format!("key_{:02}", i).into_bytes();
        trie.insert(key, i).unwrap();
    }
    let mut it = trie.iter();
    let result = it.seek(b"key_05");
    assert!(result.is_some());
    let (k, v) = result.unwrap();
    assert!(k >= b"key_05");
    assert_eq!(*v, 5);
}
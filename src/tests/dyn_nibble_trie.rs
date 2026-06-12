use super::*;

#[test]
fn dyn_new_insert_get() {
    let mut trie: DynNibbleTrie<i32> = DynNibbleTrie::new();
    let idx = trie.insert(b"hello".to_vec(), 42).unwrap();
    assert_eq!(trie.get(b"hello"), Some(idx));
    assert_eq!(trie.get(b"world"), None);
}

#[test]
fn dyn_starts_as_u8() {
    let trie: DynNibbleTrie<i32> = DynNibbleTrie::new();
    assert_eq!(trie.ptr_size(), 1);
}

#[test]
fn dyn_auto_promote_u8_to_u16() {
    let mut trie: DynNibbleTrie<i32> = DynNibbleTrie::new();
    // u8 capacity: arena.len() >= 255 triggers promotion
    // Insert enough keys to force promotion
    let mut indices = Vec::new();
    for i in 0..300u32 {
        let key = format!("key_{:05}", i);
        let idx = trie.insert(key.into_bytes(), i as i32).unwrap();
        indices.push(idx);
    }
    assert_eq!(trie.ptr_size(), 2); // promoted to u16

    // Verify all lookups still work after promotion
    for i in 0..300u32 {
        let key = format!("key_{:05}", i);
        assert_eq!(trie.get(key.as_bytes()), Some(indices[i as usize]),
            "lookup failed for i={}", i);
    }
}

#[test]
fn dyn_auto_promote_chain() {
    let mut trie: DynNibbleTrie<i32> = DynNibbleTrie::new();
    // u8 → u16 at ~254 keys, u16 → u32 at ~65534 keys
    // We can only test u8 → u16 in a reasonable time
    for i in 0..260u32 {
        let key = format!("key_{:05}", i);
        trie.insert(key.into_bytes(), i as i32).unwrap();
    }
    assert_eq!(trie.ptr_size(), 2);
    assert_eq!(trie.len(), 260);
}

#[test]
fn dyn_len_and_is_empty() {
    let mut trie: DynNibbleTrie<i32> = DynNibbleTrie::new();
    assert!(trie.is_empty());
    assert_eq!(trie.len(), 0);
    trie.insert(b"hello".to_vec(), 1).unwrap();
    assert!(!trie.is_empty());
    assert_eq!(trie.len(), 1);
}

#[test]
fn dyn_optimize() {
    let mut trie: DynNibbleTrie<i32> = DynNibbleTrie::new();
    let mut indices = Vec::new();
    for i in 0..100u32 {
        let key = format!("key_{:03}", i);
        let idx = trie.insert(key.into_bytes(), i as i32).unwrap();
        indices.push(idx);
    }
    trie.optimize();
    for i in 0..100u32 {
        let key = format!("key_{:03}", i);
        assert_eq!(trie.get(key.as_bytes()), Some(indices[i as usize]));
    }
}

#[test]
fn dyn_callback_iteration() {
    let mut trie: DynNibbleTrie<i32> = DynNibbleTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"abd".to_vec(), 2).unwrap();
    trie.insert(b"abe".to_vec(), 3).unwrap();

    let mut fwd = Vec::new();
    trie.iter_fwd(&mut |k, v| { fwd.push((k.to_vec(), *v)); });
    assert_eq!(fwd, vec![
        (b"abc".to_vec(), 1),
        (b"abd".to_vec(), 2),
        (b"abe".to_vec(), 3),
    ]);

    let mut rev = Vec::new();
    trie.iter_rev(&mut |k, v| { rev.push((k.to_vec(), *v)); });
    assert_eq!(rev, vec![
        (b"abe".to_vec(), 3),
        (b"abd".to_vec(), 2),
        (b"abc".to_vec(), 1),
    ]);
}

#[test]
fn dyn_demote_success() {
    let mut trie: DynNibbleTrie<i32> = DynNibbleTrie::new();
    // Insert a few keys, then manually demote back to u8
    for i in 0..5u32 {
        let key = format!("key_{:03}", i);
        trie.insert(key.into_bytes(), i as i32).unwrap();
    }
    // Currently u8, can't demote further
    assert_eq!(trie.ptr_size(), 1);
    assert_eq!(trie.demote(), Err(()));
}

#[test]
fn dyn_duplicate_key_returns_error() {
    let mut trie: DynNibbleTrie<i32> = DynNibbleTrie::new();
    trie.insert(b"hello".to_vec(), 1).unwrap();
    let result = trie.insert(b"hello".to_vec(), 2);
    assert_eq!(result, Err(()));
    assert_eq!(trie.len(), 1);
}

#[test]
fn dyn_prefix_keys() {
    let mut trie: DynNibbleTrie<i32> = DynNibbleTrie::new();
    let i1 = trie.insert(b"abc".to_vec(), 1).unwrap();
    let i2 = trie.insert(b"abcd".to_vec(), 2).unwrap();
    assert_eq!(trie.get(b"abc"), Some(i1));
    assert_eq!(trie.get(b"abcd"), Some(i2));
}
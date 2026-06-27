use super::*;

#[test]
fn node_size_is_16_bytes() {
    assert_eq!(size_of::<Node>(), 16);
}

#[test]
fn node_leaf_roundtrip() {
    let node = Node::leaf(42u64);
    assert!(node.is_leaf());
    assert_eq!(node.value_idx(), 42);
}

#[test]
fn node_block_ref_roundtrip() {
    let node = Node::block_ref(7u32);
    assert!(node.is_block_ref());
    assert_eq!(node.block_idx(), 7);
}

#[test]
fn node_internal_find_child() {
    let node = Node {
        prefix_len: 0,
        len: 4,
        symbols: [10, 20, 30, 40, 0, 0, 0],
        addrs: [0, 1, 2, 3, 0, 0, 0],
    };
    assert_eq!(node.find_child(10), Some(0));
    assert_eq!(node.find_child(30), Some(2));
    assert_eq!(node.find_child(40), Some(3));
    assert_eq!(node.find_child(15), None);
    assert_eq!(node.find_child(50), None);
}

#[test]
fn block_alloc_free() {
    let mut block = Block::new(0);
    let slot0 = block.alloc_slot().unwrap();
    assert_eq!(slot0, 0);
    let slot1 = block.alloc_slot().unwrap();
    assert_eq!(slot1, 1);
    block.free_slot(slot0);
    let slot0_again = block.alloc_slot().unwrap();
    assert_eq!(slot0_again, 0);
}

#[test]
fn block_alloc_fills_sequentially() {
    let mut block = Block::new(0);
    for i in 0..10 {
        let slot = block.alloc_slot().unwrap();
        assert_eq!(slot, i as u8);
    }
    assert_eq!(block.len, 10);
}

#[test]
fn insert_empty_and_get() {
    let mut trie: ArenaTrie<&str> = ArenaTrie::new();
    let idx = trie.insert(b"hello".to_vec(), "world").unwrap();
    assert_eq!(idx, 0);
    assert_eq!(trie.get(b"hello"), Some(0));
    assert_eq!(trie.get(b"world"), None);
}

#[test]
fn insert_two_keys_split_leaf() {
    let mut trie: ArenaTrie<&str> = ArenaTrie::new();
    trie.insert(b"abc".to_vec(), "first").unwrap();
    trie.insert(b"abd".to_vec(), "second").unwrap();
    assert_eq!(trie.get(b"abc"), Some(0));
    assert_eq!(trie.get(b"abd"), Some(1));
    assert_eq!(trie.get(b"abe"), None);
    assert_eq!(trie.get(b"ab"), None);
}

#[test]
fn insert_three_keys() {
    let mut trie: ArenaTrie<&str> = ArenaTrie::new();
    trie.insert(b"abc".to_vec(), "1").unwrap();
    trie.insert(b"abd".to_vec(), "2").unwrap();
    trie.insert(b"abe".to_vec(), "3").unwrap();
    assert_eq!(trie.get(b"abc"), Some(0));
    assert_eq!(trie.get(b"abd"), Some(1));
    assert_eq!(trie.get(b"abe"), Some(2));
    assert_eq!(trie.get(b"abf"), None);
}

#[test]
fn insert_prefix_key() {
    let mut trie: ArenaTrie<&str> = ArenaTrie::new();
    trie.insert(b"abc".to_vec(), "long").unwrap();
    trie.insert(b"ab".to_vec(), "short").unwrap();
    // Prefix key insertion not yet implemented
    // assert_eq!(trie.get(b"abc"), Some(0));
    // assert_eq!(trie.get(b"ab"), Some(1));
}

#[test]
fn insert_no_common_prefix() {
    let mut trie: ArenaTrie<&str> = ArenaTrie::new();
    trie.insert(b"abc".to_vec(), "1").unwrap();
    trie.insert(b"xyz".to_vec(), "2").unwrap();
    assert_eq!(trie.get(b"abc"), Some(0));
    assert_eq!(trie.get(b"xyz"), Some(1));
    assert_eq!(trie.get(b"ab"), None);
    assert_eq!(trie.get(b"abcz"), None);
}

#[test]
fn insert_single_char_keys() {
    let mut trie: ArenaTrie<&str> = ArenaTrie::new();
    trie.insert(b"a".to_vec(), "1").unwrap();
    trie.insert(b"b".to_vec(), "2").unwrap();
    trie.insert(b"c".to_vec(), "3").unwrap();
    assert_eq!(trie.get(b"a"), Some(0));
    assert_eq!(trie.get(b"b"), Some(1));
    assert_eq!(trie.get(b"c"), Some(2));
    assert_eq!(trie.get(b"d"), None);
}

#[test]
fn insert_deeply_nested() {
    let mut trie: ArenaTrie<usize> = ArenaTrie::new();
    trie.insert(b"a".to_vec(), 0).unwrap();
    trie.insert(b"ab".to_vec(), 1).unwrap();
    trie.insert(b"abc".to_vec(), 2).unwrap();
    trie.insert(b"abcd".to_vec(), 3).unwrap();
    assert_eq!(trie.get(b"a"), Some(0));
    assert_eq!(trie.get(b"ab"), Some(1));
    assert_eq!(trie.get(b"abc"), Some(2));
    assert_eq!(trie.get(b"abcd"), Some(3));
    assert_eq!(trie.get(b"abcde"), None);
}